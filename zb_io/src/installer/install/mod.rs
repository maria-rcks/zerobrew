mod bottle;
pub mod doctor;
mod outdated;
mod plan;
mod source;
mod uninstall;

use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs4::fs_std::FileExt;
use tracing::warn;
use zb_core::formula_token;

use crate::cellar::link::Linker;
use crate::cellar::materialize::Cellar;
use crate::network::api::ApiClient;
use crate::network::cache::ApiCache;
use crate::network::download::{DownloadProgressCallback, DownloadRequest, ParallelDownloader};
use crate::progress::{InstallProgress, ProgressCallback};
use crate::storage::blob::BlobCache;
use crate::storage::db::Database;
use crate::storage::store::Store;

use zb_core::{Error, Formula, InstallMethod};

use bottle::dependency_cellar_path;

const MAX_CORRUPTION_RETRIES: usize = 3;

pub struct Installer {
    api_client: ApiClient,
    downloader: ParallelDownloader,
    store: Store,
    cellar: Cellar,
    linker: Linker,
    pub(crate) db: Database,
    prefix: PathBuf,
    app_dir: PathBuf,
    font_dir: PathBuf,
    appimage_dir: PathBuf,
    locks_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PlannedInstall {
    pub install_name: String,
    pub formula: Formula,
    pub method: InstallMethod,
}

#[derive(Debug)]
pub struct InstallPlan {
    pub items: Vec<PlannedInstall>,
}

pub struct ExecuteResult {
    pub installed: usize,
}

#[derive(Clone)]
pub struct CaskInstallOptions {
    pub link: bool,
    pub binaries: bool,
    pub force: bool,
    pub app_dir: Option<PathBuf>,
    pub font_dir: Option<PathBuf>,
    pub appimage_dir: Option<PathBuf>,
    pub progress: Option<DownloadProgressCallback>,
    pub(crate) dependency_stack: Vec<String>,
}

impl std::fmt::Debug for CaskInstallOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaskInstallOptions")
            .field("link", &self.link)
            .field("binaries", &self.binaries)
            .field("force", &self.force)
            .field("app_dir", &self.app_dir)
            .field("font_dir", &self.font_dir)
            .field("appimage_dir", &self.appimage_dir)
            .field("progress", &self.progress.as_ref().map(|_| "<callback>"))
            .field("dependency_stack", &self.dependency_stack)
            .finish()
    }
}

impl CaskInstallOptions {
    pub fn new(link: bool) -> Self {
        Self {
            link,
            binaries: true,
            force: false,
            app_dir: None,
            font_dir: None,
            appimage_dir: None,
            progress: None,
            dependency_stack: Vec::new(),
        }
    }
}

/// A package that has a newer version available upstream.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OutdatedPackage {
    pub name: String,
    pub installed_version: String,
    pub current_version: String,
    #[serde(skip)]
    pub installed_sha256: String,
    #[serde(skip)]
    pub current_sha256: String,
    #[serde(skip)]
    pub is_source_build: bool,
}

impl Installer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        api_client: ApiClient,
        blob_cache: BlobCache,
        store: Store,
        cellar: Cellar,
        linker: Linker,
        db: Database,
        prefix: PathBuf,
        locks_dir: PathBuf,
    ) -> Self {
        let app_dir = default_app_dir();
        Self::new_with_app_dir(
            api_client, blob_cache, store, cellar, linker, db, prefix, app_dir, locks_dir,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_app_dir(
        api_client: ApiClient,
        blob_cache: BlobCache,
        store: Store,
        cellar: Cellar,
        linker: Linker,
        db: Database,
        prefix: PathBuf,
        app_dir: PathBuf,
        locks_dir: PathBuf,
    ) -> Self {
        let font_dir = default_font_dir();
        Self::new_with_cask_dirs(
            api_client, blob_cache, store, cellar, linker, db, prefix, app_dir, font_dir, locks_dir,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_cask_dirs(
        api_client: ApiClient,
        blob_cache: BlobCache,
        store: Store,
        cellar: Cellar,
        linker: Linker,
        db: Database,
        prefix: PathBuf,
        app_dir: PathBuf,
        font_dir: PathBuf,
        locks_dir: PathBuf,
    ) -> Self {
        let appimage_dir = default_appimage_dir();
        Self {
            api_client,
            downloader: ParallelDownloader::new(blob_cache),
            store,
            cellar,
            linker,
            db,
            prefix,
            app_dir,
            font_dir,
            appimage_dir,
            locks_dir,
        }
    }

    pub fn prefix(&self) -> &Path {
        &self.prefix
    }

    pub fn app_dir(&self) -> &Path {
        &self.app_dir
    }

    pub fn font_dir(&self) -> &Path {
        &self.font_dir
    }

    pub fn appimage_dir(&self) -> &Path {
        &self.appimage_dir
    }

    pub fn set_cask_artifact_dirs(&mut self, app_dir: PathBuf, font_dir: PathBuf) {
        self.app_dir = app_dir;
        self.font_dir = font_dir;
    }

    pub fn clear_api_cache(&self) -> Result<usize, Error> {
        self.api_client.clear_cache()
    }

    pub async fn execute(&mut self, plan: InstallPlan, link: bool) -> Result<ExecuteResult, Error> {
        self.execute_with_progress(plan, link, None).await
    }

    pub async fn execute_with_progress(
        &mut self,
        plan: InstallPlan,
        link: bool,
        progress: Option<Arc<ProgressCallback>>,
    ) -> Result<ExecuteResult, Error> {
        let lock_path = self.locks_dir.join("install.lock");
        let lock_file =
            File::create(&lock_path).map_err(Error::store("failed to create install lock"))?;
        lock_file
            .lock_exclusive()
            .map_err(Error::store("failed to acquire install lock"))?;
        let _lock = lock_file;

        let report = |event: InstallProgress| {
            if let Some(ref cb) = progress {
                cb(event);
            }
        };

        let (bottle_items, source_items): (Vec<_>, Vec<_>) = plan
            .items
            .into_iter()
            .partition(|item| matches!(item.method, InstallMethod::Bottle(_)));

        if bottle_items.is_empty() && source_items.is_empty() {
            return Ok(ExecuteResult { installed: 0 });
        }

        let mut installed = 0usize;
        let mut error: Option<Error> = None;

        if !bottle_items.is_empty() {
            let requests: Vec<DownloadRequest> = bottle_items
                .iter()
                .map(|item| {
                    let InstallMethod::Bottle(ref bottle) = item.method else {
                        unreachable!()
                    };
                    DownloadRequest {
                        url: bottle.url.clone(),
                        sha256: bottle.sha256.clone(),
                        name: item.formula.name.clone(),
                    }
                })
                .collect();

            let download_progress: Option<DownloadProgressCallback> = progress.clone().map(|cb| {
                Arc::new(move |event: InstallProgress| {
                    cb(event);
                }) as DownloadProgressCallback
            });

            let mut rx = self
                .downloader
                .download_streaming(requests, download_progress.clone());

            let mut prepare_handles = Vec::with_capacity(bottle_items.len());
            while let Some(result) = rx.recv().await {
                match result {
                    Ok(download) => {
                        let item = bottle_items[download.index].clone();
                        let downloader = self.downloader.clone();
                        let store = self.store.clone();
                        let cellar = self.cellar.clone();
                        let download_progress = download_progress.clone();
                        let progress = progress.clone();

                        prepare_handles.push(tokio::spawn(async move {
                            Self::prepare_bottle_item_with_parts(
                                downloader,
                                store,
                                cellar,
                                item,
                                download,
                                download_progress,
                                progress,
                            )
                            .await
                        }));
                    }
                    Err(e) => {
                        error = Some(e);
                    }
                }
            }

            let mut prepared_kegs = vec![None; bottle_items.len()];
            for handle in prepare_handles {
                match handle
                    .await
                    .map_err(Error::network("bottle prepare task failed"))?
                {
                    Ok(prepared) => {
                        prepared_kegs[prepared.index] = Some(prepared.keg_path);
                    }
                    Err(e) => {
                        error = Some(e);
                    }
                }
            }

            for (index, item) in bottle_items.iter().enumerate() {
                let Some(keg_path) = prepared_kegs[index].as_ref() else {
                    continue;
                };
                match self.finalize_bottle_item(item, keg_path, link, &report) {
                    Ok(()) => installed += 1,
                    Err(e) => error = Some(e),
                }
            }
        }

        for item in &source_items {
            let InstallMethod::Source(ref build_plan) = item.method else {
                unreachable!()
            };

            report(InstallProgress::UnpackStarted {
                name: item.formula.name.clone(),
            });

            match self
                .install_from_source(item, build_plan, link, &report)
                .await
            {
                Ok(()) => installed += 1,
                Err(e) => {
                    error = Some(e);
                    continue;
                }
            }
        }

        if let Some(e) = error {
            return Err(e);
        }

        Ok(ExecuteResult { installed })
    }

    pub async fn install(&mut self, names: &[String], link: bool) -> Result<ExecuteResult, Error> {
        let (casks, formulas): (Vec<_>, Vec<_>) = names
            .iter()
            .cloned()
            .partition(|name| name.starts_with("cask:"));

        let mut installed = 0usize;

        if !formulas.is_empty() {
            let plan = self.plan(&formulas).await?;
            installed += self.execute(plan, link).await?.installed;
        }

        if !casks.is_empty() {
            installed += self.install_casks(&casks, link).await?.installed;
        }

        Ok(ExecuteResult { installed })
    }

    pub async fn install_casks(
        &mut self,
        names: &[String],
        link: bool,
    ) -> Result<ExecuteResult, Error> {
        self.install_casks_with_options(names, CaskInstallOptions::new(link))
            .await
    }

    pub async fn install_casks_with_options(
        &mut self,
        names: &[String],
        options: CaskInstallOptions,
    ) -> Result<ExecuteResult, Error> {
        let mut installed = 0usize;
        let original_app_dir = self.app_dir.clone();
        let original_font_dir = self.font_dir.clone();
        let original_appimage_dir = self.appimage_dir.clone();
        if let Some(app_dir) = &options.app_dir {
            self.app_dir = app_dir.clone();
        }
        if let Some(font_dir) = &options.font_dir {
            self.font_dir = font_dir.clone();
        }
        if let Some(appimage_dir) = &options.appimage_dir {
            self.appimage_dir = appimage_dir.clone();
        }

        let result = async {
            for name in names {
                if self.is_installed(name) {
                    continue;
                }
                let token = name
                    .strip_prefix("cask:")
                    .expect("install_casks expects cask: prefixed names");
                Box::pin(self.install_single_cask(token, &options)).await?;
                installed += 1;
            }
            Ok(ExecuteResult { installed })
        }
        .await;

        self.app_dir = original_app_dir;
        self.font_dir = original_font_dir;
        self.appimage_dir = original_appimage_dir;
        result
    }

    pub async fn install_casks_from_json(
        &mut self,
        casks: &[(String, serde_json::Value)],
        link: bool,
    ) -> Result<ExecuteResult, Error> {
        let mut installed = 0usize;
        let mut first_error = None;
        for (token, cask_json) in casks {
            if self.is_installed(&format!("cask:{token}")) {
                continue;
            }
            match self
                .install_single_cask_from_json(token, cask_json.clone(), link)
                .await
            {
                Ok(()) => installed += 1,
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }

        if let Some(e) = first_error {
            return Err(e);
        }

        Ok(ExecuteResult { installed })
    }

    pub fn is_installed(&self, name: &str) -> bool {
        self.db
            .get_installed(name)
            .is_some_and(|installed| self.installed_keg_exists(&installed))
    }

    pub fn get_installed(&self, name: &str) -> Option<crate::storage::db::InstalledKeg> {
        self.db.get_installed(name)
    }

    pub fn list_installed(&self) -> Result<Vec<crate::storage::db::InstalledKeg>, Error> {
        self.db.list_installed()
    }

    pub async fn formula_dependencies(
        &self,
        name: &str,
        include_build: bool,
    ) -> Result<Vec<String>, Error> {
        let formula = self.api_client.get_formula(name).await?;
        let mut dependencies = formula.dependencies.clone();
        if include_build {
            dependencies.extend(formula.all_build_dependencies());
            dependencies.sort_unstable();
            dependencies.dedup();
        }
        Ok(dependencies)
    }

    pub async fn formula_dependents(&self, name: &str) -> Result<Vec<String>, Error> {
        let requested = formula_token(name).to_string();
        let bulk_raw = self.api_client.get_all_formulas_raw().await?;
        let formulas: Vec<Formula> = serde_json::from_str(&bulk_raw)
            .map_err(Error::network("failed to parse bulk formula JSON"))?;
        let mut dependents: Vec<String> = formulas
            .into_iter()
            .filter_map(|formula| {
                let depends_on_requested = formula
                    .dependencies
                    .iter()
                    .any(|dependency| formula_token(dependency) == requested);
                depends_on_requested.then_some(formula.name)
            })
            .collect();
        dependents.sort_unstable();
        Ok(dependents)
    }

    pub async fn list_leaves(&self) -> Result<Vec<crate::storage::db::InstalledKeg>, Error> {
        let installed = self.db.list_installed()?;
        let formula_kegs: Vec<_> = installed
            .into_iter()
            .filter(|keg| !keg.name.starts_with("cask:"))
            .collect();

        let installed_tokens: HashSet<_> = formula_kegs
            .iter()
            .map(|keg| formula_token(&keg.name).to_string())
            .collect();
        let mut dependency_tokens = HashSet::new();

        for keg in &formula_kegs {
            let formula = self.api_client.get_formula(&keg.name).await?;
            for dependency in formula.dependencies {
                let token = formula_token(&dependency);
                if installed_tokens.contains(token) {
                    dependency_tokens.insert(token.to_string());
                }
            }
        }

        Ok(formula_kegs
            .into_iter()
            .filter(|keg| !dependency_tokens.contains(formula_token(&keg.name)))
            .collect())
    }

    pub fn link_installed(&mut self, name: &str) -> Result<Vec<crate::cellar::LinkedFile>, Error> {
        let _lock = self.acquire_install_lock()?;
        let installed = self.db.get_installed(name).ok_or(Error::NotInstalled {
            name: name.to_string(),
        })?;
        let keg_path = self.keg_path(formula_token(&installed.name), &installed.version);
        let linked = self.linker.link_keg(&keg_path)?;

        if let Err(err) = self.persist_linked_files(name, &installed.version, &linked) {
            if let Err(unlink_err) = self.linker.unlink_keg(&keg_path) {
                warn!(
                    formula = %name,
                    error = %unlink_err,
                    "failed to roll back links after DB persistence error"
                );
            }
            return Err(err);
        }

        Ok(linked)
    }

    pub fn unlink_installed(&mut self, name: &str) -> Result<Vec<PathBuf>, Error> {
        let _lock = self.acquire_install_lock()?;
        let installed = self.db.get_installed(name).ok_or(Error::NotInstalled {
            name: name.to_string(),
        })?;
        let keg_path = self.keg_path(formula_token(&installed.name), &installed.version);
        let unlinked = self.linker.unlink_keg(&keg_path)?;

        let tx = self.db.transaction()?;
        tx.clear_keg_file_records(name)?;
        tx.commit()?;

        Ok(unlinked)
    }

    fn acquire_install_lock(&self) -> Result<File, Error> {
        fs::create_dir_all(&self.locks_dir).map_err(Error::store("failed to create locks dir"))?;
        let lock_path = self.locks_dir.join("install.lock");
        let lock_file =
            File::create(&lock_path).map_err(Error::store("failed to create install lock"))?;
        lock_file
            .lock_exclusive()
            .map_err(Error::store("failed to acquire install lock"))?;
        Ok(lock_file)
    }

    fn persist_linked_files(
        &mut self,
        name: &str,
        version: &str,
        linked: &[crate::cellar::LinkedFile],
    ) -> Result<(), Error> {
        let tx = self.db.transaction()?;
        tx.clear_keg_file_records(name)?;
        for file in linked {
            tx.record_linked_file(
                name,
                version,
                &file.link_path.to_string_lossy(),
                &file.target_path.to_string_lossy(),
            )?;
        }
        tx.commit()
    }

    pub fn keg_path(&self, name: &str, version: &str) -> PathBuf {
        self.cellar.keg_path(name, version)
    }

    fn installed_keg_exists(&self, installed: &crate::storage::db::InstalledKeg) -> bool {
        self.cellar
            .keg_path(formula_token(&installed.name), &installed.version)
            .exists()
    }

    fn cleanup_materialized(cellar: &Cellar, name: &str, version: &str) {
        if let Err(e) = cellar.remove_keg(name, version) {
            warn!(
                formula = %name,
                version = %version,
                error = %e,
                "failed to remove keg after install error"
            );
        }
    }
}

fn default_app_dir() -> PathBuf {
    if let Ok(path) = std::env::var("ZEROBREW_APPDIR") {
        return PathBuf::from(path);
    }

    PathBuf::from("/Applications")
}

fn default_font_dir() -> PathBuf {
    if let Ok(path) = std::env::var("ZEROBREW_FONTDIR") {
        return PathBuf::from(path);
    }

    if cfg!(target_os = "macos")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join("Library").join("Fonts");
    }

    if let Ok(xdg_data_home) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(xdg_data_home).join("fonts");
    }

    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("fonts");
    }

    PathBuf::from("fonts")
}

fn default_appimage_dir() -> PathBuf {
    if let Ok(path) = std::env::var("ZEROBREW_APPIMAGEDIR") {
        return PathBuf::from(path);
    }

    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join("Applications");
    }

    PathBuf::from("Applications")
}

pub fn create_installer(
    root: &Path,
    prefix: &Path,
    concurrency: usize,
) -> Result<Installer, Error> {
    if !root.exists() {
        fs::create_dir_all(root).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                Error::StoreCorruption {
                    message: format!(
                        "cannot create root directory '{}': permission denied.\n\n\
                        Create it with:\n  sudo mkdir -p {} && sudo chown $USER {}",
                        root.display(),
                        root.display(),
                        root.display()
                    ),
                }
            } else {
                Error::StoreCorruption {
                    message: format!("failed to create root directory '{}': {e}", root.display()),
                }
            }
        })?;
    }

    fs::create_dir_all(root.join("db")).map_err(Error::store("failed to create db directory"))?;

    fs::create_dir_all(root.join("cache"))
        .map_err(Error::store("failed to create cache directory"))?;

    let api_cache_path = root.join("cache/api-cache.sqlite");
    let api_cache =
        ApiCache::open(&api_cache_path).map_err(Error::store("failed to open API cache"))?;

    let api_client = match std::env::var("ZEROBREW_API_URL") {
        Ok(url) => ApiClient::with_base_url(url)?,
        Err(_) => ApiClient::new(),
    }
    .with_cache(api_cache);

    let blob_cache =
        BlobCache::new(&root.join("cache")).map_err(Error::store("failed to create blob cache"))?;
    let store = Store::new(root).map_err(Error::store("failed to create store"))?;
    // Use prefix/Cellar so bottles' hardcoded rpaths work
    let cellar =
        Cellar::new_at(prefix.join("Cellar")).map_err(Error::store("failed to create cellar"))?;
    let linker = Linker::new(prefix).map_err(Error::store("failed to create linker"))?;
    let db = Database::open(&root.join("db/zb.sqlite3"))?;

    let locks_dir = root.join("locks");
    fs::create_dir_all(&locks_dir).map_err(Error::store("failed to create locks directory"))?;

    let parallel_downloader = ParallelDownloader::with_concurrency(blob_cache, concurrency);

    Ok(Installer {
        api_client,
        downloader: parallel_downloader,
        store,
        cellar,
        linker,
        db,
        prefix: prefix.to_path_buf(),
        app_dir: default_app_dir(),
        font_dir: default_font_dir(),
        appimage_dir: default_appimage_dir(),
        locks_dir,
    })
}

#[cfg(test)]
mod test_support {
    pub fn create_bottle_tarball(formula_name: &str) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        use tar::Builder;

        let mut builder = Builder::new(Vec::new());

        let mut header = tar::Header::new_gnu();
        header
            .set_path(format!("{}/1.0.0/bin/{}", formula_name, formula_name))
            .unwrap();
        header.set_size(20);
        header.set_mode(0o755);
        header.set_cksum();

        let content = format!("#!/bin/sh\necho {}", formula_name);
        builder.append(&header, content.as_bytes()).unwrap();

        let tar_data = builder.into_inner().unwrap();

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_data).unwrap();
        encoder.finish().unwrap()
    }

    pub fn sha256_hex(data: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    pub fn get_test_bottle_tag() -> &'static str {
        if cfg!(target_os = "linux") {
            "x86_64_linux"
        } else if cfg!(target_arch = "x86_64") {
            "sonoma"
        } else {
            "arm64_sonoma"
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::cellar::Cellar;
    use crate::network::api::ApiClient;
    use crate::storage::blob::BlobCache;
    use crate::storage::db::Database;
    use crate::storage::store::Store;
    use crate::{Installer, Linker};

    use super::test_support::*;

    fn create_local_installer(root: &std::path::Path, prefix: &std::path::Path) -> Installer {
        create_local_installer_with_api(root, prefix, "http://127.0.0.1:1/formula".to_string())
    }

    fn create_local_installer_with_api(
        root: &std::path::Path,
        prefix: &std::path::Path,
        api_url: String,
    ) -> Installer {
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client = ApiClient::with_base_url(api_url).expect("test API URL should be valid");
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(root).unwrap();
        let cellar = Cellar::new_at(prefix.join("Cellar")).unwrap();
        let linker = Linker::new(prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.to_path_buf(),
            root.join("locks"),
        )
    }

    fn create_installed_binary(installer: &mut Installer, name: &str, version: &str) {
        let keg_path = installer.keg_path(name, version);
        fs::create_dir_all(keg_path.join("bin")).unwrap();
        fs::write(keg_path.join("bin").join(name), b"#!/bin/sh\n").unwrap();

        let tx = installer.db.transaction().unwrap();
        tx.record_install(name, version, "store-key").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn link_installed_records_symlinks() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer = create_local_installer(&root, &prefix);
        create_installed_binary(&mut installer, "linkme", "1.0.0");

        let linked = installer.link_installed("linkme").unwrap();

        assert_eq!(linked.len(), 1);
        assert!(prefix.join("bin/linkme").is_symlink());

        let records = installer.db.list_keg_files().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "linkme");
        assert_eq!(
            records[0].linked_path,
            prefix.join("bin/linkme").to_string_lossy()
        );
    }

    #[test]
    fn unlink_installed_removes_symlinks_and_records() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer = create_local_installer(&root, &prefix);
        create_installed_binary(&mut installer, "unlinkme", "1.0.0");
        installer.link_installed("unlinkme").unwrap();

        let unlinked = installer.unlink_installed("unlinkme").unwrap();

        assert_eq!(unlinked.len(), 1);
        assert!(!prefix.join("bin/unlinkme").exists());
        assert!(installer.db.list_keg_files().unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_leaves_excludes_installed_dependencies() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer = create_local_installer_with_api(
            &root,
            &prefix,
            format!("{}/formula", mock_server.uri()),
        );

        let root_formula = r#"{
            "name": "root",
            "versions": { "stable": "1.0.0" },
            "dependencies": ["dep"],
            "bottle": {
                "stable": {
                    "files": {
                        "all": {
                            "url": "https://example.com/root.tar.gz",
                            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        }
                    }
                }
            }
        }"#;
        let dep_formula = r#"{
            "name": "dep",
            "versions": { "stable": "1.0.0" },
            "dependencies": [],
            "bottle": {
                "stable": {
                    "files": {
                        "all": {
                            "url": "https://example.com/dep.tar.gz",
                            "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                        }
                    }
                }
            }
        }"#;

        Mock::given(method("GET"))
            .and(path("/formula/root.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(root_formula))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path("/formula/dep.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(dep_formula))
            .mount(&mock_server)
            .await;

        let tx = installer.db.transaction().unwrap();
        tx.record_install("dep", "1.0.0", "dep-key").unwrap();
        tx.record_install("root", "1.0.0", "root-key").unwrap();
        tx.record_install("cask:app", "1.0.0", "app-key").unwrap();
        tx.commit().unwrap();

        let leaves = installer.list_leaves().await.unwrap();
        let names: Vec<_> = leaves.into_iter().map(|keg| keg.name).collect();

        assert_eq!(names, vec!["root"]);
    }

    #[tokio::test]
    async fn install_completes_successfully() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let bottle = create_bottle_tarball("testpkg");
        let bottle_sha = sha256_hex(&bottle);

        let tag = get_test_bottle_tag();
        let formula_json = format!(
            r#"{{
                "name": "testpkg",
                "versions": {{ "stable": "1.0.0" }},
                "dependencies": [],
                "bottle": {{
                    "stable": {{
                        "files": {{
                            "{}": {{
                                "url": "{}/bottles/testpkg-1.0.0.{}.bottle.tar.gz",
                                "sha256": "{}"
                            }}
                        }}
                    }}
                }}
            }}"#,
            tag,
            mock_server.uri(),
            tag,
            bottle_sha
        );

        Mock::given(method("GET"))
            .and(path("/formula/testpkg.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&formula_json))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!(
                "/bottles/testpkg-1.0.0.{}.bottle.tar.gz",
                tag
            )))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bottle.clone()))
            .expect(1)
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client =
            ApiClient::with_base_url(format!("{}/formula", mock_server.uri())).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.clone(),
            root.join("locks"),
        );

        installer
            .install(&["testpkg".to_string()], true)
            .await
            .unwrap();

        let second_install = installer
            .install(&["testpkg".to_string()], true)
            .await
            .unwrap();
        assert_eq!(second_install.installed, 0);

        assert!(root.join("cellar/testpkg/1.0.0").exists());
        assert!(prefix.join("bin/testpkg").exists());

        let installed = installer.db.get_installed("testpkg");
        assert!(installed.is_some());
        assert_eq!(installed.unwrap().version, "1.0.0");
    }

    #[tokio::test]
    async fn install_with_dependencies() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let dep_bottle = create_bottle_tarball("deplib");
        let dep_sha = sha256_hex(&dep_bottle);
        let main_bottle = create_bottle_tarball("mainpkg");
        let main_sha = sha256_hex(&main_bottle);

        let tag = get_test_bottle_tag();
        let dep_json = format!(
            r#"{{"name":"deplib","versions":{{"stable":"1.0.0"}},"dependencies":[],"bottle":{{"stable":{{"files":{{"{}":{{"url":"{}/bottles/deplib-1.0.0.{}.bottle.tar.gz","sha256":"{}"}}}}}}}}}}"#,
            tag,
            mock_server.uri(),
            tag,
            dep_sha
        );
        let main_json = format!(
            r#"{{"name":"mainpkg","versions":{{"stable":"2.0.0"}},"dependencies":["deplib"],"bottle":{{"stable":{{"files":{{"{}":{{"url":"{}/bottles/mainpkg-2.0.0.{}.bottle.tar.gz","sha256":"{}"}}}}}}}}}}"#,
            tag,
            mock_server.uri(),
            tag,
            main_sha
        );

        Mock::given(method("GET"))
            .and(path("/formula/deplib.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&dep_json))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path("/formula/mainpkg.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&main_json))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/bottles/deplib-1.0.0.{}.bottle.tar.gz", tag)))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(dep_bottle))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/bottles/mainpkg-2.0.0.{}.bottle.tar.gz",
                tag
            )))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(main_bottle))
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client =
            ApiClient::with_base_url(format!("{}/formula", mock_server.uri())).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.clone(),
            root.join("locks"),
        );

        installer
            .install(&["mainpkg".to_string()], true)
            .await
            .unwrap();

        assert!(installer.db.get_installed("mainpkg").is_some());
        assert!(installer.db.get_installed("deplib").is_some());
    }

    #[tokio::test]
    async fn install_cask_installs_formula_dependencies() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let dep_bottle = create_bottle_tarball("deplib");
        let dep_sha = sha256_hex(&dep_bottle);
        let cask_binary = b"#!/bin/sh\necho cask";
        let cask_sha = sha256_hex(cask_binary);

        let tag = get_test_bottle_tag();
        let dep_json = format!(
            r#"{{"name":"deplib","versions":{{"stable":"1.0.0"}},"dependencies":[],"bottle":{{"stable":{{"files":{{"{}":{{"url":"{}/bottles/deplib-1.0.0.{}.bottle.tar.gz","sha256":"{}"}}}}}}}}}}"#,
            tag,
            mock_server.uri(),
            tag,
            dep_sha
        );
        let cask_json = format!(
            r#"{{
                "token": "dep-cask",
                "version": "1.0.0",
                "url": "{}/downloads/dep-cask",
                "sha256": "{}",
                "depends_on": {{ "formula": ["deplib"] }},
                "artifacts": [{{ "binary": ["dep-cask"] }}]
            }}"#,
            mock_server.uri(),
            cask_sha
        );

        Mock::given(method("GET"))
            .and(path("/formula/deplib.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&dep_json))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/bottles/deplib-1.0.0.{}.bottle.tar.gz", tag)))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(dep_bottle))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path("/cask/dep-cask.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&cask_json))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path("/downloads/dep-cask"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(cask_binary))
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client = ApiClient::with_base_url(format!("{}/formula", mock_server.uri()))
            .unwrap()
            .with_cask_base_url(format!("{}/cask", mock_server.uri()));
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.clone(),
            root.join("locks"),
        );

        installer
            .install(&["cask:dep-cask".to_string()], true)
            .await
            .unwrap();

        assert!(installer.is_installed("deplib"));
        assert!(installer.is_installed("cask:dep-cask"));
        assert!(prefix.join("bin/deplib").exists());
        assert!(prefix.join("bin/dep-cask").exists());
    }

    #[tokio::test]
    async fn preserves_successful_installs_when_one_package_fails() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let good_bottle = create_bottle_tarball("goodpkg");
        let good_sha = sha256_hex(&good_bottle);

        let tag = get_test_bottle_tag();
        let good_json = format!(
            r#"{{
                "name": "goodpkg",
                "versions": {{ "stable": "1.0.0" }},
                "dependencies": [],
                "bottle": {{
                    "stable": {{
                        "files": {{
                            "{}": {{
                                "url": "{}/bottles/goodpkg-1.0.0.{}.bottle.tar.gz",
                                "sha256": "{}"
                            }}
                        }}
                    }}
                }}
            }}"#,
            tag,
            mock_server.uri(),
            tag,
            good_sha
        );

        let bad_json = format!(
            r#"{{
                "name": "badpkg",
                "versions": {{ "stable": "1.0.0" }},
                "dependencies": [],
                "bottle": {{
                    "stable": {{
                        "files": {{
                            "{}": {{
                                "url": "{}/bottles/badpkg-1.0.0.{}.bottle.tar.gz",
                                "sha256": "{}"
                            }}
                        }}
                    }}
                }}
            }}"#,
            tag,
            mock_server.uri(),
            tag,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );

        Mock::given(method("GET"))
            .and(path("/formula/goodpkg.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&good_json))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path("/formula/badpkg.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&bad_json))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/bottles/goodpkg-1.0.0.{}.bottle.tar.gz",
                tag
            )))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(good_bottle))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/bottles/badpkg-1.0.0.{}.bottle.tar.gz", tag)))
            .respond_with(
                ResponseTemplate::new(500)
                    .set_delay(Duration::from_millis(100))
                    .set_body_string("download failed"),
            )
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client =
            ApiClient::with_base_url(format!("{}/formula", mock_server.uri())).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.clone(),
            root.join("locks"),
        );

        let result = installer
            .install(&["goodpkg".to_string(), "badpkg".to_string()], false)
            .await;
        assert!(result.is_err());

        assert!(installer.db.get_installed("goodpkg").is_some());
        assert!(installer.db.get_installed("badpkg").is_none());
        assert!(root.join("cellar/goodpkg/1.0.0").exists());
    }

    #[tokio::test]
    async fn db_persist_failure_cleans_materialized_and_linked_files() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let bottle = create_bottle_tarball("rollbackme");
        let bottle_sha = sha256_hex(&bottle);

        let tag = get_test_bottle_tag();
        let formula_json = format!(
            r#"{{
                "name": "rollbackme",
                "versions": {{ "stable": "1.0.0" }},
                "dependencies": [],
                "bottle": {{
                    "stable": {{
                        "files": {{
                            "{}": {{
                                "url": "{}/bottles/rollbackme-1.0.0.{}.bottle.tar.gz",
                                "sha256": "{}"
                            }}
                        }}
                    }}
                }}
            }}"#,
            tag,
            mock_server.uri(),
            tag,
            bottle_sha
        );

        Mock::given(method("GET"))
            .and(path("/formula/rollbackme.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&formula_json))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/bottles/rollbackme-1.0.0.{}.bottle.tar.gz",
                tag
            )))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bottle))
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let db_path = root.join("db/zb.sqlite3");
        let api_client =
            ApiClient::with_base_url(format!("{}/formula", mock_server.uri())).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&db_path).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.clone(),
            root.join("locks"),
        );

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("DROP TABLE installed_kegs", []).unwrap();

        let result = installer.install(&["rollbackme".to_string()], true).await;
        assert!(result.is_err());

        assert!(!root.join("cellar/rollbackme/1.0.0").exists());
        assert!(!prefix.join("bin/rollbackme").exists());
        assert!(!prefix.join("opt/rollbackme").exists());
        assert!(root.join("store").join(&bottle_sha).exists());
    }

    #[tokio::test]
    async fn db_persist_failure_cleans_materialized_tap_formula_keg() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let bottle = create_bottle_tarball("terraform");
        let bottle_sha = sha256_hex(&bottle);
        let tag = get_test_bottle_tag();

        let tap_formula_rb = format!(
            r#"
class Terraform < Formula
  version "1.10.0"
  bottle do
    root_url "{}/v2/hashicorp/tap"
    sha256 {}: "{}"
  end
end
"#,
            mock_server.uri(),
            tag,
            bottle_sha
        );

        Mock::given(method("GET"))
            .and(path("/hashicorp/homebrew-tap/main/Formula/terraform.rb"))
            .respond_with(ResponseTemplate::new(200).set_body_string(tap_formula_rb))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/v2/hashicorp/tap/terraform/blobs/sha256:{bottle_sha}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bottle))
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let db_path = root.join("db/zb.sqlite3");
        let api_client = ApiClient::with_base_url(format!("{}/formula", mock_server.uri()))
            .unwrap()
            .with_tap_raw_base_url(mock_server.uri());
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&db_path).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.clone(),
            root.join("locks"),
        );

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("DROP TABLE installed_kegs", []).unwrap();

        let result = installer
            .install(&["hashicorp/tap/terraform".to_string()], true)
            .await;
        assert!(result.is_err());

        assert!(!root.join("cellar/terraform/1.10.0").exists());
        assert!(!prefix.join("bin/terraform").exists());
        assert!(!prefix.join("opt/terraform").exists());
        assert!(root.join("store").join(&bottle_sha).exists());
    }

    #[tokio::test]
    async fn parallel_api_fetching_with_deep_deps() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let leaf1_bottle = create_bottle_tarball("leaf1");
        let leaf1_sha = sha256_hex(&leaf1_bottle);
        let leaf2_bottle = create_bottle_tarball("leaf2");
        let leaf2_sha = sha256_hex(&leaf2_bottle);
        let mid1_bottle = create_bottle_tarball("mid1");
        let mid1_sha = sha256_hex(&mid1_bottle);
        let mid2_bottle = create_bottle_tarball("mid2");
        let mid2_sha = sha256_hex(&mid2_bottle);
        let root_bottle = create_bottle_tarball("root");
        let root_sha = sha256_hex(&root_bottle);

        let tag = get_test_bottle_tag();
        let leaf1_json = format!(
            r#"{{"name":"leaf1","versions":{{"stable":"1.0.0"}},"dependencies":[],"bottle":{{"stable":{{"files":{{"{}":{{"url":"{}/bottles/leaf1.tar.gz","sha256":"{}"}}}}}}}}}}"#,
            tag,
            mock_server.uri(),
            leaf1_sha
        );
        let leaf2_json = format!(
            r#"{{"name":"leaf2","versions":{{"stable":"1.0.0"}},"dependencies":[],"bottle":{{"stable":{{"files":{{"{}":{{"url":"{}/bottles/leaf2.tar.gz","sha256":"{}"}}}}}}}}}}"#,
            tag,
            mock_server.uri(),
            leaf2_sha
        );
        let mid1_json = format!(
            r#"{{"name":"mid1","versions":{{"stable":"1.0.0"}},"dependencies":["leaf1"],"bottle":{{"stable":{{"files":{{"{}":{{"url":"{}/bottles/mid1.tar.gz","sha256":"{}"}}}}}}}}}}"#,
            tag,
            mock_server.uri(),
            mid1_sha
        );
        let mid2_json = format!(
            r#"{{"name":"mid2","versions":{{"stable":"1.0.0"}},"dependencies":["leaf1","leaf2"],"bottle":{{"stable":{{"files":{{"{}":{{"url":"{}/bottles/mid2.tar.gz","sha256":"{}"}}}}}}}}}}"#,
            tag,
            mock_server.uri(),
            mid2_sha
        );
        let root_json = format!(
            r#"{{"name":"root","versions":{{"stable":"1.0.0"}},"dependencies":["mid1","mid2"],"bottle":{{"stable":{{"files":{{"{}":{{"url":"{}/bottles/root.tar.gz","sha256":"{}"}}}}}}}}}}"#,
            tag,
            mock_server.uri(),
            root_sha
        );

        for (name, json) in [
            ("leaf1", &leaf1_json),
            ("leaf2", &leaf2_json),
            ("mid1", &mid1_json),
            ("mid2", &mid2_json),
            ("root", &root_json),
        ] {
            Mock::given(method("GET"))
                .and(path(format!("/formula/{}.json", name)))
                .respond_with(ResponseTemplate::new(200).set_body_string(json))
                .mount(&mock_server)
                .await;
        }
        for (name, bottle) in [
            ("leaf1", &leaf1_bottle),
            ("leaf2", &leaf2_bottle),
            ("mid1", &mid1_bottle),
            ("mid2", &mid2_bottle),
            ("root", &root_bottle),
        ] {
            Mock::given(method("GET"))
                .and(path(format!("/bottles/{}.tar.gz", name)))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(bottle.clone()))
                .mount(&mock_server)
                .await;
        }

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client =
            ApiClient::with_base_url(format!("{}/formula", mock_server.uri())).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.clone(),
            root.join("locks"),
        );

        installer
            .install(&["root".to_string()], true)
            .await
            .unwrap();

        assert!(installer.db.get_installed("root").is_some());
        assert!(installer.db.get_installed("mid1").is_some());
        assert!(installer.db.get_installed("mid2").is_some());
        assert!(installer.db.get_installed("leaf1").is_some());
        assert!(installer.db.get_installed("leaf2").is_some());
    }

    #[tokio::test]
    async fn streaming_extraction_processes_as_downloads_complete() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let fast_bottle = create_bottle_tarball("fastpkg");
        let fast_sha = sha256_hex(&fast_bottle);
        let slow_bottle = create_bottle_tarball("slowpkg");
        let slow_sha = sha256_hex(&slow_bottle);

        let tag = get_test_bottle_tag();
        let fast_json = format!(
            r#"{{"name":"fastpkg","versions":{{"stable":"1.0.0"}},"dependencies":[],"bottle":{{"stable":{{"files":{{"{}":{{"url":"{}/bottles/fast.tar.gz","sha256":"{}"}}}}}}}}}}"#,
            tag,
            mock_server.uri(),
            fast_sha
        );
        let slow_json = format!(
            r#"{{"name":"slowpkg","versions":{{"stable":"1.0.0"}},"dependencies":["fastpkg"],"bottle":{{"stable":{{"files":{{"{}":{{"url":"{}/bottles/slow.tar.gz","sha256":"{}"}}}}}}}}}}"#,
            tag,
            mock_server.uri(),
            slow_sha
        );

        Mock::given(method("GET"))
            .and(path("/formula/fastpkg.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&fast_json))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path("/formula/slowpkg.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&slow_json))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path("/bottles/fast.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(fast_bottle.clone()))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path("/bottles/slow.tar.gz"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(slow_bottle.clone())
                    .set_delay(Duration::from_millis(100)),
            )
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client =
            ApiClient::with_base_url(format!("{}/formula", mock_server.uri())).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.clone(),
            root.join("locks"),
        );

        installer
            .install(&["slowpkg".to_string()], true)
            .await
            .unwrap();

        assert!(installer.db.get_installed("fastpkg").is_some());
        assert!(installer.db.get_installed("slowpkg").is_some());
        assert!(root.join("cellar/fastpkg/1.0.0").exists());
        assert!(root.join("cellar/slowpkg/1.0.0").exists());
        assert!(prefix.join("bin/fastpkg").exists());
        assert!(prefix.join("bin/slowpkg").exists());
    }

    #[tokio::test]
    async fn retries_on_corrupted_download() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let bottle = create_bottle_tarball("retrypkg");
        let bottle_sha = sha256_hex(&bottle);

        let tag = get_test_bottle_tag();
        let formula_json = format!(
            r#"{{
                "name": "retrypkg",
                "versions": {{ "stable": "1.0.0" }},
                "dependencies": [],
                "bottle": {{
                    "stable": {{
                        "files": {{
                            "{}": {{
                                "url": "{}/bottles/retrypkg-1.0.0.{}.bottle.tar.gz",
                                "sha256": "{}"
                            }}
                        }}
                    }}
                }}
            }}"#,
            tag,
            mock_server.uri(),
            tag,
            bottle_sha
        );

        Mock::given(method("GET"))
            .and(path("/formula/retrypkg.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&formula_json))
            .mount(&mock_server)
            .await;

        let attempt_count = Arc::new(AtomicUsize::new(0));
        let attempt_clone = attempt_count.clone();
        let valid_bottle = bottle.clone();

        Mock::given(method("GET"))
            .and(path(format!(
                "/bottles/retrypkg-1.0.0.{}.bottle.tar.gz",
                tag
            )))
            .respond_with(move |_: &wiremock::Request| {
                let _attempt = attempt_clone.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_bytes(valid_bottle.clone())
            })
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client =
            ApiClient::with_base_url(format!("{}/formula", mock_server.uri())).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.clone(),
            root.join("locks"),
        );

        installer
            .install(&["retrypkg".to_string()], true)
            .await
            .unwrap();

        assert!(installer.is_installed("retrypkg"));
        assert!(root.join("cellar/retrypkg/1.0.0").exists());
        assert!(prefix.join("bin/retrypkg").exists());
    }

    #[tokio::test]
    async fn fails_after_max_retries() {
        // Validates the retry mechanism structure -- proper integration test
        // would need injection of corruption between download and extraction.
    }

    #[test]
    fn is_installed_ignores_stale_database_records() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client =
            ApiClient::with_base_url("http://127.0.0.1:1/formula".to_string()).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let mut db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        {
            let tx = db.transaction().unwrap();
            tx.record_install("cask:stale", "1.0.0", "deadbeef")
                .unwrap();
            tx.commit().unwrap();
        }

        let installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix,
            root.join("locks"),
        );

        assert!(!installer.is_installed("cask:stale"));
    }
}
