use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, Semaphore, mpsc};

use crate::checksum::normalize_sha256;
use crate::storage::blob::BlobCache;
use zb_core::Error;

use super::single::Downloader;
use super::{DownloadProgressCallback, DownloadResult, GLOBAL_DOWNLOAD_CONCURRENCY};

pub struct DownloadRequest {
    pub url: String,
    pub sha256: String,
    pub name: String,
}

type InflightMap = HashMap<String, Arc<tokio::sync::broadcast::Sender<Result<PathBuf, String>>>>;

#[derive(Clone)]
pub struct ParallelDownloader {
    downloader: Arc<Downloader>,
    inflight: Arc<Mutex<InflightMap>>,
}

impl ParallelDownloader {
    pub fn new(blob_cache: BlobCache) -> Self {
        let semaphore = Arc::new(Semaphore::new(GLOBAL_DOWNLOAD_CONCURRENCY));
        Self {
            downloader: Arc::new(Downloader::with_semaphore(blob_cache, Some(semaphore))),
            inflight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_concurrency(blob_cache: BlobCache, concurrency: usize) -> Self {
        let semaphore = Arc::new(Semaphore::new(concurrency));
        Self {
            downloader: Arc::new(Downloader::with_semaphore(blob_cache, Some(semaphore))),
            inflight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn remove_blob(&self, sha256: &str) -> bool {
        self.downloader.remove_blob(sha256)
    }

    pub async fn download_single(
        &self,
        request: DownloadRequest,
        progress: Option<DownloadProgressCallback>,
    ) -> Result<PathBuf, Error> {
        Self::download_with_dedup(
            self.downloader.clone(),
            self.inflight.clone(),
            request,
            progress,
        )
        .await
    }

    pub async fn download_all(
        &self,
        requests: Vec<DownloadRequest>,
    ) -> Result<Vec<PathBuf>, Error> {
        self.download_all_with_progress(requests, None).await
    }

    pub async fn download_all_with_progress(
        &self,
        requests: Vec<DownloadRequest>,
        progress: Option<DownloadProgressCallback>,
    ) -> Result<Vec<PathBuf>, Error> {
        let handles: Vec<_> = requests
            .into_iter()
            .map(|req| {
                let downloader = self.downloader.clone();
                let inflight = self.inflight.clone();
                let progress = progress.clone();

                tokio::spawn(async move {
                    Self::download_with_dedup(downloader, inflight, req, progress).await
                })
            })
            .collect();

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            let result = handle.await.map_err(Error::network("task join error"))??;
            results.push(result);
        }

        Ok(results)
    }

    pub fn download_streaming(
        &self,
        requests: Vec<DownloadRequest>,
        progress: Option<DownloadProgressCallback>,
    ) -> mpsc::Receiver<Result<DownloadResult, Error>> {
        let (tx, rx) = mpsc::channel(requests.len().max(1));

        for (index, req) in requests.into_iter().enumerate() {
            let downloader = self.downloader.clone();
            let inflight = self.inflight.clone();
            let progress = progress.clone();
            let tx = tx.clone();
            let name = req.name.clone();
            let sha256 = req.sha256.clone();

            tokio::spawn(async move {
                let result = Self::download_with_dedup(downloader, inflight, req, progress).await;
                let _ = tx
                    .send(result.map(|blob_path| DownloadResult {
                        name,
                        sha256,
                        blob_path,
                        index,
                    }))
                    .await;
            });
        }

        rx
    }

    async fn download_with_dedup(
        downloader: Arc<Downloader>,
        inflight: Arc<Mutex<InflightMap>>,
        mut req: DownloadRequest,
        progress: Option<DownloadProgressCallback>,
    ) -> Result<PathBuf, Error> {
        req.sha256 = normalize_sha256(&req.sha256)?;

        let mut receiver = {
            let mut map = inflight.lock().await;

            if let Some(sender) = map.get(&req.sha256) {
                Some(sender.subscribe())
            } else {
                let (tx, _) = tokio::sync::broadcast::channel(1);
                map.insert(req.sha256.clone(), Arc::new(tx));
                None
            }
        };

        if let Some(ref mut rx) = receiver {
            let result = rx
                .recv()
                .await
                .map_err(Error::network("broadcast recv error"))?;

            return result.map_err(|msg| Error::NetworkFailure { message: msg });
        }

        let result = downloader
            .download_with_progress(&req.url, &req.sha256, Some(req.name), progress)
            .await;

        {
            let mut map = inflight.lock().await;
            if let Some(sender) = map.remove(&req.sha256) {
                let broadcast_result = match &result {
                    Ok(path) => Ok(path.clone()),
                    Err(e) => Err(e.to_string()),
                };
                let _ = sender.send(broadcast_result);
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use sha2::{Digest, Sha256};
    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::storage::blob::BlobCache;

    use super::super::GLOBAL_DOWNLOAD_CONCURRENCY;
    use super::*;

    #[tokio::test]
    async fn peak_concurrent_downloads_within_limit() {
        let mock_server = MockServer::start().await;
        let concurrent_count = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        let content = b"test content";
        let count_clone = concurrent_count.clone();
        let max_clone = max_concurrent.clone();

        Mock::given(method("GET"))
            .respond_with(move |_: &wiremock::Request| {
                let current = count_clone.fetch_add(1, Ordering::SeqCst) + 1;
                max_clone.fetch_max(current, Ordering::SeqCst);

                std::thread::sleep(Duration::from_millis(50));

                count_clone.fetch_sub(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_bytes(content.to_vec())
            })
            .mount(&mock_server)
            .await;

        let tmp = TempDir::new().unwrap();
        let blob_cache = BlobCache::new(tmp.path()).unwrap();
        let downloader = ParallelDownloader::new(blob_cache);

        let requests: Vec<_> = (0..5)
            .map(|i| {
                let sha256 = format!("{:064x}", i);
                DownloadRequest {
                    url: format!("{}/file{i}.tar.gz", mock_server.uri()),
                    sha256,
                    name: format!("pkg{i}"),
                }
            })
            .collect();

        let _ = downloader.download_all(requests).await;

        let peak = max_concurrent.load(Ordering::SeqCst);
        assert!(
            peak <= GLOBAL_DOWNLOAD_CONCURRENCY,
            "peak concurrent downloads was {peak}, expected <= {GLOBAL_DOWNLOAD_CONCURRENCY}"
        );
    }

    #[tokio::test]
    async fn same_blob_requested_multiple_times_fetches_once() {
        let mock_server = MockServer::start().await;
        let content = b"deduplicated content";

        let actual_sha256 = {
            let mut hasher = Sha256::new();
            hasher.update(content);
            format!("{:x}", hasher.finalize())
        };

        Mock::given(method("GET"))
            .and(path("/dedup.tar.gz"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(content.to_vec())
                    .set_delay(Duration::from_millis(100)),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let tmp = TempDir::new().unwrap();
        let blob_cache = BlobCache::new(tmp.path()).unwrap();
        let downloader = ParallelDownloader::new(blob_cache);

        let requests: Vec<_> = (0..5)
            .map(|i| DownloadRequest {
                url: format!("{}/dedup.tar.gz", mock_server.uri()),
                sha256: actual_sha256.clone(),
                name: format!("dedup{i}"),
            })
            .collect();

        let results = downloader.download_all(requests).await.unwrap();

        assert_eq!(results.len(), 5);
        for path in &results {
            assert!(path.exists());
        }
    }

    #[tokio::test]
    async fn chunked_downloads_do_not_exhaust_global_permits() {
        let mock_server = MockServer::start().await;
        let contents = Arc::new(vec![
            vec![0xA0u8; 12 * 1024 * 1024],
            vec![0xB0u8; 12 * 1024 * 1024],
        ]);

        let shas: Vec<_> = contents
            .iter()
            .map(|content| {
                let mut hasher = Sha256::new();
                hasher.update(content);
                format!("{:x}", hasher.finalize())
            })
            .collect();

        let head_contents = contents.clone();
        Mock::given(method("HEAD"))
            .respond_with(move |req: &wiremock::Request| {
                let index = req
                    .url
                    .path()
                    .strip_prefix("/large")
                    .and_then(|path| path.strip_suffix(".tar.gz"))
                    .and_then(|index| index.parse::<usize>().ok());

                if let Some(index) = index
                    && let Some(content) = head_contents.get(index)
                {
                    return ResponseTemplate::new(200)
                        .append_header("Accept-Ranges", "bytes")
                        .append_header("Content-Length", content.len().to_string());
                }

                ResponseTemplate::new(404)
            })
            .mount(&mock_server)
            .await;

        let get_contents = contents.clone();
        Mock::given(method("GET"))
            .respond_with(move |req: &wiremock::Request| {
                let index = req
                    .url
                    .path()
                    .strip_prefix("/large")
                    .and_then(|path| path.strip_suffix(".tar.gz"))
                    .and_then(|index| index.parse::<usize>().ok());

                let Some(content) = index.and_then(|index| get_contents.get(index)) else {
                    return ResponseTemplate::new(404);
                };

                if let Some(range_header) = req.headers.get("Range") {
                    let range_str = range_header.to_str().unwrap();
                    let range_part = range_str.strip_prefix("bytes=").unwrap();
                    let (start_str, end_str) = range_part.split_once('-').unwrap();
                    let start: usize = start_str.parse().unwrap();
                    let end: usize = end_str.parse().unwrap();
                    let chunk = &content[start..=end];

                    return ResponseTemplate::new(206)
                        .append_header("Content-Length", chunk.len().to_string())
                        .append_header(
                            "Content-Range",
                            format!("bytes {}-{}/{}", start, end, content.len()),
                        )
                        .set_body_bytes(chunk.to_vec());
                }

                ResponseTemplate::new(200).set_body_bytes(content.clone())
            })
            .mount(&mock_server)
            .await;

        let tmp = TempDir::new().unwrap();
        let blob_cache = BlobCache::new(tmp.path()).unwrap();
        let downloader = ParallelDownloader::with_concurrency(blob_cache, 2);

        let requests: Vec<_> = (0..2)
            .map(|index| DownloadRequest {
                url: format!("{}/large{index}.tar.gz", mock_server.uri()),
                sha256: shas[index].clone(),
                name: format!("large{index}"),
            })
            .collect();

        let result =
            tokio::time::timeout(Duration::from_secs(15), downloader.download_all(requests))
                .await
                .expect("chunked downloads should not deadlock waiting for permits");
        let paths = result.unwrap();

        assert_eq!(paths.len(), contents.len());
        for (path, expected) in paths.iter().zip(contents.iter()) {
            assert_eq!(std::fs::read(path).unwrap(), *expected);
        }
    }
}
