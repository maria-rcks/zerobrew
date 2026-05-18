use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::progress::InstallProgress;
use crate::storage::blob::BlobCache;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT_RANGES, AUTHORIZATION, CONTENT_RANGE};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use zb_core::Error;

use super::auth::{
    TokenCache, bearer_header, fetch_bearer_token_internal, fetch_download_response_internal,
    fetch_range_response_internal, get_cached_token_for_url_internal,
};
use super::single::download_response_internal;
use super::{DownloadProgressCallback, MAX_CHUNK_RETRIES, MAX_CONCURRENT_CHUNKS};
use crate::checksum::normalize_sha256;

const MIN_CHUNK_SIZE: u64 = 5 * 1024 * 1024;
const MAX_CHUNK_SIZE: u64 = 20 * 1024 * 1024;

struct ChunkDownloadContext<'a> {
    client: &'a reqwest::Client,
    token_cache: &'a TokenCache,
    url: &'a str,
    progress: Option<DownloadProgressCallback>,
    name: Option<String>,
    file_size: u64,
    total_downloaded: Arc<AtomicU64>,
}

pub(crate) struct ChunkedDownloadContext<'a> {
    pub(crate) blob_cache: &'a BlobCache,
    pub(crate) client: &'a reqwest::Client,
    pub(crate) token_cache: &'a TokenCache,
    pub(crate) url: &'a str,
    pub(crate) expected_sha256: &'a str,
    pub(crate) name: Option<String>,
    pub(crate) progress: Option<DownloadProgressCallback>,
    pub(crate) file_size: u64,
    pub(crate) global_semaphore: &'a Arc<Semaphore>,
}

struct ChunkRange {
    offset: u64,
    size: u64,
}

struct CompletedChunk {
    offset: u64,
    data: Vec<u8>,
}

pub(crate) fn server_supports_ranges(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("bytes"))
        .unwrap_or(false)
}

fn calculate_chunk_size(file_size: u64) -> u64 {
    let target_chunks = MAX_CONCURRENT_CHUNKS as u64;
    let chunk_size = file_size / target_chunks;
    chunk_size.clamp(MIN_CHUNK_SIZE, MAX_CHUNK_SIZE)
}

fn calculate_chunk_ranges(file_size: u64) -> Vec<ChunkRange> {
    let chunk_size = calculate_chunk_size(file_size);
    let mut chunks = Vec::new();
    let mut offset = 0;

    while offset < file_size {
        let remaining = file_size - offset;
        let chunk_size = remaining.min(chunk_size);
        chunks.push(ChunkRange {
            offset,
            size: chunk_size,
        });
        offset += chunk_size;
    }

    chunks
}

async fn download_chunk(
    ctx: &ChunkDownloadContext<'_>,
    chunk: &ChunkRange,
) -> Result<CompletedChunk, Error> {
    let range_header = format!("bytes={}-{}", chunk.offset, chunk.offset + chunk.size - 1);

    let mut last_error = None;

    for attempt in 0..=MAX_CHUNK_RETRIES {
        let cached_token = get_cached_token_for_url_internal(ctx.token_cache, ctx.url).await;

        let mut request = ctx
            .client
            .get(ctx.url)
            .header("Range", range_header.clone());
        if let Some(token) = &cached_token {
            request = request.header(AUTHORIZATION, bearer_header(token)?);
        }

        match request.send().await {
            Ok(response) => {
                if response.status() == StatusCode::UNAUTHORIZED {
                    let www_auth = match response.headers().get(reqwest::header::WWW_AUTHENTICATE) {
                        Some(value) => value.to_str().map_err(|_| Error::NetworkFailure {
                            message: "WWW-Authenticate header contains invalid characters"
                                .to_string(),
                        })?,
                        None => {
                            return Err(Error::NetworkFailure {
                                message: "server returned 401 without WWW-Authenticate header"
                                    .to_string(),
                            });
                        }
                    };

                    match fetch_bearer_token_internal(ctx.client, ctx.token_cache, www_auth).await {
                        Ok(_new_token) => {
                            last_error = Some(Error::NetworkFailure {
                                message: "token expired, retrying with new token".to_string(),
                            });
                            continue;
                        }
                        Err(e) => {
                            return Err(Error::network("failed to refresh token")(e));
                        }
                    }
                }

                if let Some(content_range) = response.headers().get(CONTENT_RANGE) {
                    let range_str = content_range.to_str().unwrap_or("");
                    if !range_str.contains(&format!(
                        "{}-{}",
                        chunk.offset,
                        chunk.offset + chunk.size - 1
                    )) {
                        return Err(Error::NetworkFailure {
                            message: format!(
                                "invalid content-range: expected bytes {}-{}, got: {}",
                                chunk.offset,
                                chunk.offset + chunk.size - 1,
                                range_str
                            ),
                        });
                    }
                }

                if !response.status().is_success() {
                    let err = Error::NetworkFailure {
                        message: format!("chunk download returned HTTP {}", response.status()),
                    };

                    if response.status().is_server_error() && attempt < MAX_CHUNK_RETRIES {
                        last_error = Some(err);
                        tokio::time::sleep(Duration::from_millis(100 * (1 << attempt))).await;
                        continue;
                    }
                    return Err(err);
                }

                let mut chunk_data = Vec::with_capacity(chunk.size as usize);
                let mut stream = response.bytes_stream();
                let mut written = 0u64;

                while let Some(item) = stream.next().await {
                    let bytes = item.map_err(Error::network("failed to read chunk bytes"))?;
                    written += bytes.len() as u64;
                    chunk_data.extend_from_slice(&bytes);

                    if let (Some(cb), Some(n)) = (&ctx.progress, &ctx.name) {
                        let downloaded = ctx
                            .total_downloaded
                            .fetch_add(bytes.len() as u64, Ordering::Release);
                        cb(InstallProgress::DownloadProgress {
                            name: n.clone(),
                            downloaded: downloaded + bytes.len() as u64,
                            total_bytes: Some(ctx.file_size),
                        });
                    }
                }

                if written != chunk.size {
                    return Err(Error::NetworkFailure {
                        message: format!(
                            "chunk size mismatch: expected {} bytes, got {} bytes",
                            chunk.size, written
                        ),
                    });
                }

                return Ok(CompletedChunk {
                    offset: chunk.offset,
                    data: chunk_data,
                });
            }
            Err(e) => {
                last_error = Some(Error::network("chunk download failed")(e));

                if attempt < MAX_CHUNK_RETRIES {
                    tokio::time::sleep(Duration::from_millis(100 * (1 << attempt))).await;
                    continue;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| Error::NetworkFailure {
        message: "chunk download failed after retries".to_string(),
    }))
}

fn spawn_chunk_download(
    ctx: &ChunkedDownloadContext<'_>,
    chunk: ChunkRange,
    total_downloaded: Arc<AtomicU64>,
) -> tokio::task::JoinHandle<Result<CompletedChunk, Error>> {
    let client = ctx.client.clone();
    let token_cache = ctx.token_cache.clone();
    let url = ctx.url.to_string();
    let global_semaphore = Arc::clone(ctx.global_semaphore);
    let progress = ctx.progress.clone();
    let name = ctx.name.clone();
    let file_size = ctx.file_size;

    tokio::spawn(async move {
        let _permit = global_semaphore
            .acquire()
            .await
            .map_err(Error::network("global semaphore error"))?;

        let chunk_ctx = ChunkDownloadContext {
            client: &client,
            token_cache: &token_cache,
            url: &url,
            progress,
            name,
            file_size,
            total_downloaded,
        };

        download_chunk(&chunk_ctx, &chunk).await
    })
}

pub(crate) async fn download_with_chunks(
    ctx: &ChunkedDownloadContext<'_>,
) -> Result<PathBuf, Error> {
    let expected_sha256 = normalize_sha256(ctx.expected_sha256)?;

    if !validate_range_support(ctx).await? {
        let response =
            fetch_download_response_internal(ctx.client, ctx.token_cache, ctx.url).await?;
        return download_response_internal(
            ctx.blob_cache,
            response,
            &expected_sha256,
            ctx.name.clone(),
            ctx.progress.clone(),
        )
        .await;
    }

    let chunks = calculate_chunk_ranges(ctx.file_size);

    if let (Some(cb), Some(n)) = (&ctx.progress, &ctx.name) {
        cb(InstallProgress::DownloadStarted {
            name: n.clone(),
            total_bytes: Some(ctx.file_size),
        });
    }

    let mut writer = ctx
        .blob_cache
        .start_write(&expected_sha256)
        .map_err(Error::network("failed to create blob writer"))?;

    let total_chunks = chunks.len();
    let total_downloaded = Arc::new(AtomicU64::new(0));
    let mut chunks = chunks.into_iter();
    let mut pending = FuturesUnordered::new();

    for _ in 0..MAX_CONCURRENT_CHUNKS {
        let Some(chunk) = chunks.next() else {
            break;
        };
        pending.push(spawn_chunk_download(ctx, chunk, total_downloaded.clone()));
    }

    let mut buffered_chunks: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    let mut chunks_written = 0u64;
    let mut next_hash_offset = 0u64;
    let mut hasher = Sha256::new();

    while let Some(result) = pending.next().await {
        let completed = match result {
            Ok(Ok(completed)) => {
                if let Some(next_chunk) = chunks.next() {
                    pending.push(spawn_chunk_download(
                        ctx,
                        next_chunk,
                        total_downloaded.clone(),
                    ));
                }
                completed
            }
            Ok(Err(err)) => {
                for handle in pending.iter() {
                    handle.abort();
                }
                return Err(err);
            }
            Err(err) => {
                for handle in pending.iter() {
                    handle.abort();
                }
                return Err(Error::network("chunk download task failed")(err));
            }
        };

        writer
            .seek(std::io::SeekFrom::Start(completed.offset))
            .map_err(|e| Error::NetworkFailure {
                message: format!("failed to seek to offset {}: {e}", completed.offset),
            })?;
        writer
            .write_all(&completed.data)
            .map_err(|e| Error::NetworkFailure {
                message: format!("failed to write chunk at offset {}: {e}", completed.offset),
            })?;

        if buffered_chunks
            .insert(completed.offset, completed.data)
            .is_some()
        {
            return Err(Error::NetworkFailure {
                message: format!("received duplicate chunk at offset {}", completed.offset),
            });
        }

        while let Some(data) = buffered_chunks.remove(&next_hash_offset) {
            hasher.update(&data);
            next_hash_offset += data.len() as u64;
            chunks_written += 1;
        }
    }

    if chunks_written as usize != total_chunks {
        return Err(Error::NetworkFailure {
            message: format!(
                "expected {} chunks, received {}",
                total_chunks, chunks_written
            ),
        });
    }

    if next_hash_offset != ctx.file_size {
        return Err(Error::NetworkFailure {
            message: format!(
                "incomplete write: expected {} bytes, wrote {} bytes",
                ctx.file_size, next_hash_offset
            ),
        });
    }

    writer
        .flush()
        .map_err(Error::network("failed to flush download"))?;

    let actual_hash = format!("{:x}", hasher.finalize());

    if actual_hash != expected_sha256 {
        return Err(Error::ChecksumMismatch {
            expected: expected_sha256,
            actual: actual_hash,
        });
    }

    if let (Some(cb), Some(n)) = (&ctx.progress, &ctx.name) {
        cb(InstallProgress::DownloadCompleted {
            name: n.clone(),
            total_bytes: ctx.file_size,
        });
    }

    writer.commit()
}

async fn validate_range_support(ctx: &ChunkedDownloadContext<'_>) -> Result<bool, Error> {
    let response =
        fetch_range_response_internal(ctx.client, ctx.token_cache, ctx.url, "bytes=0-0").await?;

    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Ok(false);
    }

    let content_range = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    Ok(content_range.contains("0-0"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use sha2::{Digest, Sha256};
    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::storage::blob::BlobCache;

    use super::super::single::Downloader;
    use super::MAX_CONCURRENT_CHUNKS;
    use std::sync::Arc;

    #[tokio::test]
    async fn chunked_download_for_large_files() {
        let mock_server = MockServer::start().await;

        let large_content = vec![0xABu8; 15 * 1024 * 1024];
        let actual_sha256 = {
            let mut hasher = Sha256::new();
            hasher.update(&large_content);
            format!("{:x}", hasher.finalize())
        };

        Mock::given(method("HEAD"))
            .and(path("/large.tar.gz"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("Accept-Ranges", "Bytes")
                    .append_header("Content-Length", large_content.len().to_string()),
            )
            .mount(&mock_server)
            .await;

        let range_requests = Arc::new(AtomicUsize::new(0));
        let range_requests_clone = range_requests.clone();
        let large_content_for_closure = large_content.clone();

        Mock::given(method("GET"))
            .and(path("/large.tar.gz"))
            .respond_with(move |req: &wiremock::Request| {
                if let Some(range_header) = req.headers.get("Range") {
                    range_requests_clone.fetch_add(1, Ordering::SeqCst);

                    let range_str = range_header.to_str().unwrap();
                    let range_part = range_str.strip_prefix("bytes=").unwrap();
                    let (start_str, end_str) = range_part.split_once('-').unwrap();
                    let start: usize = start_str.parse().unwrap();
                    let end: usize = end_str.parse().unwrap();

                    let chunk = &large_content_for_closure[start..=end];
                    ResponseTemplate::new(206)
                        .append_header("Content-Length", chunk.len().to_string())
                        .append_header(
                            "Content-Range",
                            format!(
                                "bytes {}-{}/{}",
                                start,
                                end,
                                large_content_for_closure.len()
                            ),
                        )
                        .set_body_bytes(chunk.to_vec())
                } else {
                    ResponseTemplate::new(200).set_body_bytes(large_content_for_closure.clone())
                }
            })
            .mount(&mock_server)
            .await;

        let tmp = TempDir::new().unwrap();
        let blob_cache = BlobCache::new(tmp.path()).unwrap();
        let downloader = Downloader::new(blob_cache);

        let url = format!("{}/large.tar.gz", mock_server.uri());
        let uppercase_sha256 = actual_sha256.to_uppercase();
        let result = downloader.download(&url, &uppercase_sha256).await;

        assert!(result.is_ok(), "Download failed: {:?}", result.err());
        let blob_path = result.unwrap();
        assert!(blob_path.exists());

        let range_count = range_requests.load(Ordering::SeqCst);
        assert!(
            range_count > 0,
            "Expected multiple Range requests, got {}",
            range_count
        );

        let downloaded_content = std::fs::read(&blob_path).unwrap();
        assert_eq!(downloaded_content.len(), large_content.len());
        assert_eq!(downloaded_content, large_content);
    }

    #[tokio::test]
    async fn fallback_to_normal_download_when_ranges_not_supported() {
        let mock_server = MockServer::start().await;

        let large_content = vec![0xCDu8; 15 * 1024 * 1024];
        let actual_sha256 = {
            let mut hasher = Sha256::new();
            hasher.update(&large_content);
            format!("{:x}", hasher.finalize())
        };

        Mock::given(method("HEAD"))
            .and(path("/large.tar.gz"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("Content-Length", large_content.len().to_string()),
            )
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/large.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(large_content.clone()))
            .mount(&mock_server)
            .await;

        let tmp = TempDir::new().unwrap();
        let blob_cache = BlobCache::new(tmp.path()).unwrap();
        let downloader = Downloader::new(blob_cache);

        let url = format!("{}/large.tar.gz", mock_server.uri());
        let result = downloader.download(&url, &actual_sha256).await;

        assert!(result.is_ok());
        let blob_path = result.unwrap();
        assert!(blob_path.exists());

        let downloaded_content = std::fs::read(&blob_path).unwrap();
        assert_eq!(downloaded_content, large_content);
    }

    #[tokio::test]
    async fn small_files_dont_use_chunked_download() {
        let mock_server = MockServer::start().await;

        let small_content = vec![0xEFu8; 1024 * 1024];
        let actual_sha256 = {
            let mut hasher = Sha256::new();
            hasher.update(&small_content);
            format!("{:x}", hasher.finalize())
        };

        Mock::given(method("HEAD"))
            .and(path("/small.tar.gz"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("Accept-Ranges", "bytes")
                    .append_header("Content-Length", small_content.len().to_string()),
            )
            .mount(&mock_server)
            .await;

        let range_used = Arc::new(AtomicUsize::new(0));
        let range_used_clone = range_used.clone();
        let small_content_for_closure = small_content.clone();

        Mock::given(method("GET"))
            .and(path("/small.tar.gz"))
            .respond_with(move |req: &wiremock::Request| {
                if req.headers.get("Range").is_some() {
                    range_used_clone.fetch_add(1, Ordering::SeqCst);
                }
                ResponseTemplate::new(200).set_body_bytes(small_content_for_closure.clone())
            })
            .mount(&mock_server)
            .await;

        let tmp = TempDir::new().unwrap();
        let blob_cache = BlobCache::new(tmp.path()).unwrap();
        let downloader = Downloader::new(blob_cache);

        let url = format!("{}/small.tar.gz", mock_server.uri());
        let result = downloader.download(&url, &actual_sha256).await;

        assert!(result.is_ok());
        let blob_path = result.unwrap();
        assert!(blob_path.exists());

        let range_count = range_used.load(Ordering::SeqCst);
        assert_eq!(
            range_count, 0,
            "Small files should not use chunked download"
        );

        let downloaded_content = std::fs::read(&blob_path).unwrap();
        assert_eq!(downloaded_content, small_content);
    }

    #[tokio::test]
    async fn chunked_download_respects_concurrency_limit() {
        let mock_server = MockServer::start().await;

        fn byte_at(index: u64) -> u8 {
            (index % 251) as u8
        }

        let file_size = 125_u64 * 1024 * 1024;
        let mut hasher = Sha256::new();
        let mut offset = 0;
        while offset < file_size {
            let len = (file_size - offset).min(1024 * 1024);
            let bytes: Vec<_> = (offset..offset + len).map(byte_at).collect();
            hasher.update(&bytes);
            offset += len;
        }
        let actual_sha256 = format!("{:x}", hasher.finalize());

        Mock::given(method("HEAD"))
            .and(path("/large.tar.gz"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("Accept-Ranges", "bytes")
                    .append_header("Content-Length", file_size.to_string()),
            )
            .mount(&mock_server)
            .await;

        let concurrent_count = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));
        let concurrent_clone = concurrent_count.clone();
        let max_clone = max_concurrent.clone();

        Mock::given(method("GET"))
            .and(path("/large.tar.gz"))
            .respond_with(move |req: &wiremock::Request| {
                if let Some(range_header) = req.headers.get("Range") {
                    let current = concurrent_clone.fetch_add(1, Ordering::SeqCst) + 1;
                    max_clone.fetch_max(current, Ordering::SeqCst);

                    let range_str = range_header.to_str().unwrap();
                    let range_part = range_str.strip_prefix("bytes=").unwrap();
                    let (start_str, end_str) = range_part.split_once('-').unwrap();
                    let start: usize = start_str.parse().unwrap();
                    let end: usize = end_str.parse().unwrap();

                    std::thread::sleep(Duration::from_millis(50));

                    let chunk: Vec<_> = (start as u64..=end as u64).map(byte_at).collect();

                    concurrent_clone.fetch_sub(1, Ordering::SeqCst);

                    ResponseTemplate::new(206)
                        .append_header("Content-Length", chunk.len().to_string())
                        .append_header(
                            "Content-Range",
                            format!("bytes {}-{}/{}", start, end, file_size),
                        )
                        .set_body_bytes(chunk)
                } else {
                    ResponseTemplate::new(200)
                }
            })
            .mount(&mock_server)
            .await;

        let tmp = TempDir::new().unwrap();
        let blob_cache = BlobCache::new(tmp.path()).unwrap();
        let downloader = Downloader::new(blob_cache);

        let url = format!("{}/large.tar.gz", mock_server.uri());
        let result = downloader.download(&url, &actual_sha256).await;

        assert!(result.is_ok(), "Download failed: {:?}", result.err());
        let blob_path = result.unwrap();
        assert!(blob_path.exists());

        let peak = max_concurrent.load(Ordering::SeqCst);
        assert!(
            peak <= MAX_CONCURRENT_CHUNKS,
            "Peak concurrent downloads was {peak}, expected <= {MAX_CONCURRENT_CHUNKS}"
        );

        let downloaded_content = std::fs::read(&blob_path).unwrap();
        assert_eq!(downloaded_content.len(), file_size as usize);
    }

    #[tokio::test]
    async fn chunk_retry_logic_succeeds_after_transient_failure() {
        let mock_server = MockServer::start().await;

        let large_content = vec![0xABu8; 15 * 1024 * 1024];
        let actual_sha256 = {
            let mut hasher = Sha256::new();
            hasher.update(&large_content);
            format!("{:x}", hasher.finalize())
        };

        Mock::given(method("HEAD"))
            .and(path("/large.tar.gz"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("Accept-Ranges", "bytes")
                    .append_header("Content-Length", large_content.len().to_string()),
            )
            .mount(&mock_server)
            .await;

        let attempt_count = Arc::new(AtomicUsize::new(0));
        let attempt_count_clone = attempt_count.clone();
        let large_content_for_closure = large_content.clone();

        Mock::given(method("GET"))
            .and(path("/large.tar.gz"))
            .respond_with(move |req: &wiremock::Request| {
                if let Some(range_header) = req.headers.get("Range") {
                    let current_attempt = attempt_count_clone.fetch_add(1, Ordering::SeqCst);

                    if current_attempt == 0 {
                        return ResponseTemplate::new(500);
                    }

                    let range_str = range_header.to_str().unwrap();
                    let range_part = range_str.strip_prefix("bytes=").unwrap();
                    let (start_str, end_str) = range_part.split_once('-').unwrap();
                    let start: usize = start_str.parse().unwrap();
                    let end: usize = end_str.parse().unwrap();

                    let chunk = &large_content_for_closure[start..=end];
                    ResponseTemplate::new(206)
                        .append_header("Content-Length", chunk.len().to_string())
                        .append_header(
                            "Content-Range",
                            format!(
                                "bytes {}-{}/{}",
                                start,
                                end,
                                large_content_for_closure.len()
                            ),
                        )
                        .set_body_bytes(chunk.to_vec())
                } else {
                    ResponseTemplate::new(200).set_body_bytes(large_content_for_closure.clone())
                }
            })
            .mount(&mock_server)
            .await;

        let tmp = TempDir::new().unwrap();
        let blob_cache = BlobCache::new(tmp.path()).unwrap();
        let downloader = Downloader::new(blob_cache);

        let url = format!("{}/large.tar.gz", mock_server.uri());
        let result = downloader.download(&url, &actual_sha256).await;

        assert!(result.is_ok(), "Download should succeed after retry");
        let blob_path = result.unwrap();
        assert!(blob_path.exists());

        let total_attempts = attempt_count.load(Ordering::SeqCst);
        assert!(
            total_attempts > 3,
            "Expected retry to occur (attempts: {})",
            total_attempts
        );

        let downloaded_content = std::fs::read(&blob_path).unwrap();
        assert_eq!(downloaded_content, large_content);
    }

    #[tokio::test]
    async fn auth_token_refresh_during_chunked_download() {
        let mock_server = MockServer::start().await;

        let large_content = vec![0xCDu8; 15 * 1024 * 1024];
        let actual_sha256 = {
            let mut hasher = Sha256::new();
            hasher.update(&large_content);
            format!("{:x}", hasher.finalize())
        };

        Mock::given(method("HEAD"))
            .and(path("/v2/homebrew/core/test/blobs/sha256:abc"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("Accept-Ranges", "bytes")
                    .append_header("Content-Length", large_content.len().to_string()),
            )
            .mount(&mock_server)
            .await;

        let auth_challenges = Arc::new(AtomicUsize::new(0));
        let auth_challenges_clone = auth_challenges.clone();
        let large_content_for_closure = large_content.clone();

        Mock::given(method("GET"))
            .and(path("/v2/homebrew/core/test/blobs/sha256:abc"))
            .respond_with(move |req: &wiremock::Request| {
                if let Some(range_header) = req.headers.get("Range") {
                    if req.headers.get("Authorization").is_none() {
                        let count = auth_challenges_clone.fetch_add(1, Ordering::SeqCst);
                        if count == 0 {
                            return ResponseTemplate::new(401).append_header(
                                "WWW-Authenticate",
                                "Bearer realm=\"https://ghcr.io/token\",service=\"ghcr.io\",scope=\"repository:homebrew/core/test:pull\"",
                            );
                        }
                    }

                    let range_str = range_header.to_str().unwrap();
                    let range_part = range_str.strip_prefix("bytes=").unwrap();
                    let (start_str, end_str) = range_part.split_once('-').unwrap();
                    let start: usize = start_str.parse().unwrap();
                    let end: usize = end_str.parse().unwrap();

                    let chunk = &large_content_for_closure[start..=end];
                    ResponseTemplate::new(206)
                        .append_header("Content-Length", chunk.len().to_string())
                        .append_header(
                            "Content-Range",
                            format!(
                                "bytes {}-{}/{}",
                                start,
                                end,
                                large_content_for_closure.len()
                            ),
                        )
                        .set_body_bytes(chunk.to_vec())
                } else {
                    ResponseTemplate::new(200).set_body_bytes(large_content_for_closure.clone())
                }
            })
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "token": "test-token-12345"
            })))
            .mount(&mock_server)
            .await;

        let tmp = TempDir::new().unwrap();
        let blob_cache = BlobCache::new(tmp.path()).unwrap();
        let downloader = Downloader::new(blob_cache);

        let url = format!(
            "{}/v2/homebrew/core/test/blobs/sha256:abc",
            mock_server.uri()
        );
        let result = downloader.download(&url, &actual_sha256).await;

        assert!(result.is_ok(), "Download should succeed after auth refresh");
        let blob_path = result.unwrap();
        assert!(blob_path.exists());

        let challenges = auth_challenges.load(Ordering::SeqCst);
        assert!(
            challenges > 0,
            "Expected at least one auth challenge (got {})",
            challenges
        );

        let downloaded_content = std::fs::read(&blob_path).unwrap();
        assert_eq!(downloaded_content, large_content);
    }

    #[tokio::test]
    async fn fallback_to_single_connection_on_chunk_failure() {
        let mock_server = MockServer::start().await;

        let large_content = vec![0xEFu8; 15 * 1024 * 1024];
        let actual_sha256 = {
            let mut hasher = Sha256::new();
            hasher.update(&large_content);
            format!("{:x}", hasher.finalize())
        };

        Mock::given(method("HEAD"))
            .and(path("/large.tar.gz"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("Accept-Ranges", "bytes")
                    .append_header("Content-Length", large_content.len().to_string()),
            )
            .mount(&mock_server)
            .await;

        let range_requests = Arc::new(AtomicUsize::new(0));
        let range_requests_clone = range_requests.clone();
        let large_content_for_closure = large_content.clone();

        Mock::given(method("GET"))
            .and(path("/large.tar.gz"))
            .respond_with(move |req: &wiremock::Request| {
                if let Some(range_header) = req.headers.get("Range") {
                    range_requests_clone.fetch_add(1, Ordering::SeqCst);

                    if range_header.to_str().unwrap() == "bytes=0-0" {
                        return ResponseTemplate::new(206)
                            .append_header("Content-Length", "1")
                            .append_header(
                                "Content-Range",
                                format!("bytes 0-0/{}", large_content_for_closure.len()),
                            )
                            .set_body_bytes(vec![large_content_for_closure[0]]);
                    }

                    ResponseTemplate::new(500)
                } else {
                    ResponseTemplate::new(200).set_body_bytes(large_content_for_closure.clone())
                }
            })
            .mount(&mock_server)
            .await;

        let tmp = TempDir::new().unwrap();
        let blob_cache = BlobCache::new(tmp.path()).unwrap();
        let downloader = Downloader::new(blob_cache);

        let url = format!("{}/large.tar.gz", mock_server.uri());
        let result = downloader.download(&url, &actual_sha256).await;

        assert!(
            result.is_ok(),
            "Download should succeed via fallback: {:?}",
            result.err()
        );
        let blob_path = result.unwrap();
        assert!(blob_path.exists());

        let downloaded_content = std::fs::read(&blob_path).unwrap();
        assert_eq!(downloaded_content, large_content);
    }

    #[tokio::test]
    async fn incorrect_content_range_triggers_fallback() {
        let mock_server = MockServer::start().await;

        let large_content = vec![0x12u8; 15 * 1024 * 1024];
        let actual_sha256 = {
            let mut hasher = Sha256::new();
            hasher.update(&large_content);
            format!("{:x}", hasher.finalize())
        };

        Mock::given(method("HEAD"))
            .and(path("/large.tar.gz"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("Accept-Ranges", "bytes")
                    .append_header("Content-Length", large_content.len().to_string()),
            )
            .mount(&mock_server)
            .await;

        let large_content_for_closure = large_content.clone();

        Mock::given(method("GET"))
            .and(path("/large.tar.gz"))
            .respond_with(move |req: &wiremock::Request| {
                if let Some(range_header) = req.headers.get("Range") {
                    let range_str = range_header.to_str().unwrap();

                    if range_str == "bytes=0-0" {
                        return ResponseTemplate::new(206)
                            .append_header("Content-Length", "1")
                            .append_header(
                                "Content-Range",
                                format!("bytes 0-0/{}", large_content_for_closure.len()),
                            )
                            .set_body_bytes(vec![large_content_for_closure[0]]);
                    }

                    let range_part = range_str.strip_prefix("bytes=").unwrap();
                    let (start_str, end_str) = range_part.split_once('-').unwrap();
                    let start: usize = start_str.parse().unwrap();
                    let end: usize = end_str.parse().unwrap();

                    let chunk = &large_content_for_closure[start..=end];
                    ResponseTemplate::new(206)
                        .append_header("Content-Length", chunk.len().to_string())
                        .append_header(
                            "Content-Range",
                            format!(
                                "bytes 0-{}/{}",
                                chunk.len() - 1,
                                large_content_for_closure.len()
                            ),
                        )
                        .set_body_bytes(chunk.to_vec())
                } else {
                    ResponseTemplate::new(200).set_body_bytes(large_content_for_closure.clone())
                }
            })
            .mount(&mock_server)
            .await;

        let tmp = TempDir::new().unwrap();
        let blob_cache = BlobCache::new(tmp.path()).unwrap();
        let downloader = Downloader::new(blob_cache);

        let url = format!("{}/large.tar.gz", mock_server.uri());
        let result = downloader.download(&url, &actual_sha256).await;

        assert!(
            result.is_ok(),
            "Download should succeed via fallback after incorrect Content-Range: {:?}",
            result.err()
        );
        let blob_path = result.unwrap();
        assert!(blob_path.exists());

        let downloaded_content = std::fs::read(&blob_path).unwrap();
        assert_eq!(downloaded_content, large_content);
    }
}
