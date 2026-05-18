use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "macos")]
use std::process::Stdio;
use std::sync::Arc;

use tracing::warn;
use zb_core::{Error, InstallMethod, formula_token};

use crate::cellar::link::{LinkedFile, Linker};
use crate::cellar::materialize::Cellar;
use crate::installer::cask::{CaskInstallerKind, resolve_cask};
use crate::network::download::{
    DownloadProgressCallback, DownloadRequest, DownloadResult, ParallelDownloader,
};
use crate::progress::{InstallProgress, ProgressCallback};
use crate::storage::store::Store;

use super::{CaskInstallOptions, Installer, MAX_CORRUPTION_RETRIES, PlannedInstall};

const CASK_APPS_DIR: &str = "Applications";
const CASK_FONTS_DIR: &str = "Fonts";
const CASK_PKGS_DIR: &str = "Pkgs";
const CASK_GENERIC_ARTIFACTS_DIR: &str = "Artifacts";
const CASK_APP_IMAGES_DIR: &str = "AppImages";

pub(super) struct PreparedBottle {
    pub(super) index: usize,
    pub(super) keg_path: PathBuf,
}

impl Installer {
    pub(super) fn finalize_bottle_item(
        &mut self,
        item: &PlannedInstall,
        keg_path: &Path,
        link: bool,
        report: &impl Fn(InstallProgress),
    ) -> Result<(), Error> {
        let InstallMethod::Bottle(ref bottle) = item.method else {
            unreachable!()
        };
        let install_name = &item.install_name;
        let formula_name = &item.formula.name;
        let version = item.formula.effective_version();
        let store_key = &bottle.sha256;

        run_builtin_post_install(&self.prefix, install_name, keg_path).inspect_err(|_| {
            Self::cleanup_materialized(&self.cellar, formula_name, &version);
        })?;

        let tx = self.db.transaction().inspect_err(|_| {
            Self::cleanup_materialized(&self.cellar, formula_name, &version);
        })?;

        tx.record_install(install_name, &version, store_key)
            .inspect_err(|_| {
                Self::cleanup_materialized(&self.cellar, formula_name, &version);
            })?;

        tx.commit().inspect_err(|_| {
            Self::cleanup_materialized(&self.cellar, formula_name, &version);
        })?;

        if let Err(e) = self.linker.link_opt(keg_path) {
            warn!(formula = %install_name, error = %e, "failed to create opt link");
        }
        for alias in &item.formula.aliases {
            if let Err(e) = self.linker.link_opt_alias(alias, keg_path) {
                warn!(formula = %install_name, alias = %alias, error = %e, "failed to create opt alias link");
            }
        }

        if link && !item.formula.is_keg_only() {
            report(InstallProgress::LinkStarted {
                name: formula_name.clone(),
            });
            match self.linker.link_keg(keg_path) {
                Ok(linked_files) => {
                    report(InstallProgress::LinkCompleted {
                        name: formula_name.clone(),
                    });
                    self.record_linked_files(install_name, &version, &linked_files);
                }
                Err(e) => {
                    let _ = self.linker.unlink_keg(keg_path);
                    report(InstallProgress::InstallCompleted {
                        name: formula_name.clone(),
                    });
                    return Err(e);
                }
            }
        } else if link && item.formula.is_keg_only() {
            let reason = match &item.formula.keg_only {
                zb_core::KegOnly::Reason(s) => s.clone(),
                _ if formula_name.contains('@') => "versioned formula".to_string(),
                _ => "keg-only formula".to_string(),
            };
            report(InstallProgress::LinkSkipped {
                name: formula_name.clone(),
                reason,
            });
        }

        report(InstallProgress::InstallCompleted {
            name: formula_name.clone(),
        });

        Ok(())
    }

    pub(super) async fn prepare_bottle_item_with_parts(
        downloader: ParallelDownloader,
        store: Store,
        cellar: Cellar,
        item: PlannedInstall,
        download: DownloadResult,
        download_progress: Option<DownloadProgressCallback>,
        progress: Option<Arc<ProgressCallback>>,
    ) -> Result<PreparedBottle, Error> {
        let InstallMethod::Bottle(ref bottle) = item.method else {
            unreachable!()
        };
        let formula_name = item.formula.name.clone();
        let version = item.formula.effective_version();

        if let Some(cb) = &progress {
            cb(InstallProgress::UnpackStarted {
                name: formula_name.clone(),
            });
        }

        let mut blob_path = download.blob_path.clone();
        let mut last_error = None;
        let mut store_entry = None;

        for attempt in 0..MAX_CORRUPTION_RETRIES {
            let store_for_extract = store.clone();
            let sha256 = bottle.sha256.clone();
            let blob_path_for_extract = blob_path.clone();
            match tokio::task::spawn_blocking(move || {
                store_for_extract.ensure_entry(&sha256, &blob_path_for_extract)
            })
            .await
            .map_err(Error::network("store extraction task failed"))?
            {
                Ok(entry) => {
                    store_entry = Some(entry);
                    break;
                }
                Err(Error::StoreCorruption { message }) => {
                    downloader.remove_blob(&bottle.sha256);

                    if attempt + 1 < MAX_CORRUPTION_RETRIES {
                        warn!(
                            formula = %formula_name,
                            attempt = attempt + 2,
                            max_retries = MAX_CORRUPTION_RETRIES,
                            "corrupted download detected; retrying"
                        );

                        let request = DownloadRequest {
                            url: bottle.url.clone(),
                            sha256: bottle.sha256.clone(),
                            name: formula_name.clone(),
                        };

                        match downloader
                            .download_single(request, download_progress.clone())
                            .await
                        {
                            Ok(new_path) => blob_path = new_path,
                            Err(e) => {
                                last_error = Some(e);
                                break;
                            }
                        }
                    } else {
                        last_error = Some(Error::StoreCorruption {
                            message: format!(
                                "{message}\n\nFailed after {MAX_CORRUPTION_RETRIES} attempts. The download may be corrupted at the source."
                            ),
                        });
                    }
                }
                Err(e) => {
                    last_error = Some(e);
                    break;
                }
            }
        }

        let store_entry = store_entry.ok_or_else(|| {
            last_error.unwrap_or_else(|| Error::StoreCorruption {
                message: "extraction failed with unknown error".to_string(),
            })
        })?;

        let cellar_for_materialize = cellar.clone();
        let materialize_name = formula_name.clone();
        let materialize_version = version.clone();
        let keg_path = tokio::task::spawn_blocking(move || {
            cellar_for_materialize.materialize(
                &materialize_name,
                &materialize_version,
                &store_entry,
            )
        })
        .await
        .map_err(Error::network("cellar materialization task failed"))??;

        if let Some(cb) = &progress {
            cb(InstallProgress::UnpackCompleted {
                name: formula_name.clone(),
            });
        }

        Ok(PreparedBottle {
            index: download.index,
            keg_path,
        })
    }

    fn record_linked_files(
        &mut self,
        name: &str,
        version: &str,
        linked_files: &[crate::cellar::link::LinkedFile],
    ) {
        if let Ok(tx) = self.db.transaction() {
            let mut ok = true;
            for linked in linked_files {
                if tx
                    .record_linked_file(
                        name,
                        version,
                        &linked.link_path.to_string_lossy(),
                        &linked.target_path.to_string_lossy(),
                    )
                    .is_err()
                {
                    ok = false;
                    break;
                }
            }
            if ok {
                let _ = tx.commit();
            }
        }
    }

    pub(super) fn cleanup_failed_install(
        linker: &Linker,
        cellar: &Cellar,
        name: &str,
        version: &str,
        keg_path: &Path,
        appimage_dir: &Path,
        unlink: bool,
    ) {
        if unlink && let Err(e) = linker.unlink_keg(keg_path) {
            warn!(
                formula = %name,
                version = %version,
                error = %e,
                "failed to clean up links after install error"
            );
        }

        if unlink && let Err(e) = uninstall_cask_apps(keg_path) {
            warn!(
                formula = %name,
                version = %version,
                error = %e,
                "failed to remove installed apps after install error"
            );
        }
        if unlink && let Err(e) = uninstall_cask_fonts(keg_path) {
            warn!(
                formula = %name,
                version = %version,
                error = %e,
                "failed to remove installed fonts after install error"
            );
        }

        if unlink && let Err(e) = uninstall_cask_generic_artifacts(keg_path) {
            warn!(
                formula = %name,
                version = %version,
                error = %e,
                "failed to remove installed generic artifacts after install error"
            );
        }
        if unlink && let Err(e) = uninstall_cask_app_images(keg_path, appimage_dir) {
            warn!(
                formula = %name,
                version = %version,
                error = %e,
                "failed to remove installed appimages after install error"
            );
        }
        if let Err(e) = cellar.remove_keg(name, version) {
            warn!(
                formula = %name,
                version = %version,
                error = %e,
                "failed to remove keg after install error"
            );
        }
    }

    pub(super) async fn install_single_cask(
        &mut self,
        token: &str,
        options: &CaskInstallOptions,
    ) -> Result<(), Error> {
        let cask_json = self.api_client.get_cask(token).await?;
        self.install_single_cask_from_json_with_options(token, cask_json, options)
            .await
    }

    pub(super) async fn install_single_cask_from_json(
        &mut self,
        token: &str,
        cask_json: serde_json::Value,
        link: bool,
    ) -> Result<(), Error> {
        self.install_single_cask_from_json_with_options(
            token,
            cask_json,
            &CaskInstallOptions::new(link),
        )
        .await
    }

    pub(super) async fn install_single_cask_from_json_with_options(
        &mut self,
        token: &str,
        cask_json: serde_json::Value,
        options: &CaskInstallOptions,
    ) -> Result<(), Error> {
        let mut cask = resolve_cask(token, &cask_json)?;
        self.install_cask_dependencies(&cask, options).await?;
        if !options.binaries {
            cask.binaries.clear();
        }
        let progress = options.progress.clone();
        let report = |event: InstallProgress| {
            if let Some(ref cb) = progress {
                cb(event);
            }
        };

        let blob_path = self
            .downloader
            .download_single(
                DownloadRequest {
                    url: cask.url.clone(),
                    sha256: cask.sha256.clone(),
                    name: cask.install_name.clone(),
                },
                progress.clone(),
            )
            .await?;

        let keg_path = self.cellar.keg_path(&cask.install_name, &cask.version);
        let mut cleanup = FailedInstallGuard::new(
            &self.linker,
            &self.cellar,
            &cask.install_name,
            &cask.version,
            &keg_path,
            &self.appimage_dir,
            options.link,
        );

        report(InstallProgress::UnpackStarted {
            name: cask.install_name.clone(),
        });

        if crate::extraction::is_archive(&blob_path)? {
            let extracted = self.store.ensure_entry(&cask.sha256, &blob_path)?;
            if cask.stage_only {
                copy_path_recursive(&extracted, &keg_path)?;
            }
            stage_cask_artifacts(&extracted, &keg_path, &cask)?;
        } else if is_dmg_cask(&blob_path, &cask) {
            let extracted = ensure_dmg_store_entry(&self.store, &cask.sha256, &blob_path)?;
            if cask.stage_only {
                copy_path_recursive(&extracted, &keg_path)?;
            }
            stage_cask_artifacts(&extracted, &keg_path, &cask)?;
        } else if is_raw_pkg_cask(&blob_path, &cask) {
            let stored = ensure_raw_cask_pkg_entry(&self.store, &cask.sha256, &blob_path, &cask)?;
            copy_path_recursive(&stored, &keg_path)?;
        } else if is_raw_appimage_cask(&blob_path, &cask) {
            let stored =
                ensure_raw_cask_appimage_entry(&self.store, &cask.sha256, &blob_path, &cask)?;
            copy_path_recursive(&stored, &keg_path)?;
        } else {
            let stored = ensure_raw_cask_store_entry(&self.store, &cask.sha256, &blob_path, &cask)?;
            copy_path_recursive(&stored, &keg_path)?;
        }

        report(InstallProgress::UnpackCompleted {
            name: cask.install_name.clone(),
        });

        let linked_files = if options.link {
            report(InstallProgress::LinkStarted {
                name: cask.install_name.clone(),
            });
            let linked_files = link_keg_with_force(&self.linker, &keg_path, options.force)?;
            link_cask_apps(&keg_path, self.app_dir(), &cask, options.force)?;
            link_cask_fonts(&keg_path, self.font_dir(), &cask, options.force)?;
            link_cask_generic_artifacts(&keg_path, self.prefix(), &cask, options.force)?;
            link_cask_app_images(&keg_path, self.appimage_dir(), &cask, options.force)?;
            run_cask_pkgs(&keg_path, &cask)?;
            run_cask_installers(&keg_path, self.prefix(), &cask)?;
            report(InstallProgress::LinkCompleted {
                name: cask.install_name.clone(),
            });
            linked_files
        } else {
            report(InstallProgress::LinkSkipped {
                name: cask.install_name.clone(),
                reason: "--no-link".to_string(),
            });
            Vec::new()
        };

        let tx = self.db.transaction()?;
        tx.record_install(&cask.install_name, &cask.version, &cask.sha256)?;
        for linked in &linked_files {
            tx.record_linked_file(
                &cask.install_name,
                &cask.version,
                &linked.link_path.to_string_lossy(),
                &linked.target_path.to_string_lossy(),
            )?;
        }
        tx.commit()?;

        cleanup.disarm();
        report(InstallProgress::InstallCompleted {
            name: cask.install_name.clone(),
        });
        Ok(())
    }

    async fn install_cask_dependencies(
        &mut self,
        cask: &crate::installer::cask::ResolvedCask,
        options: &CaskInstallOptions,
    ) -> Result<(), Error> {
        if options
            .dependency_stack
            .iter()
            .any(|token| token == &cask.token)
        {
            return Err(Error::InvalidArgument {
                message: format!(
                    "cask dependency cycle detected: {} -> {}",
                    options.dependency_stack.join(" -> "),
                    cask.token
                ),
            });
        }

        if !cask.depends_on_formulas.is_empty() {
            let plan = self.plan(&cask.depends_on_formulas).await?;
            self.execute(plan, true).await?;
        }

        for dependency in &cask.depends_on_casks {
            let install_name = format!("cask:{dependency}");
            if self.is_installed(&install_name) {
                continue;
            }
            let mut dependency_options = options.clone();
            dependency_options.dependency_stack.push(cask.token.clone());
            Box::pin(self.install_casks_with_options(&[install_name], dependency_options)).await?;
        }

        Ok(())
    }
}

pub(super) fn dependency_cellar_path(
    cellar: &Cellar,
    installed_name: &str,
    version: &str,
) -> String {
    cellar
        .keg_path(formula_token(installed_name), version)
        .display()
        .to_string()
}

fn link_keg_with_force(
    linker: &Linker,
    keg_path: &Path,
    force: bool,
) -> Result<Vec<LinkedFile>, Error> {
    match linker.link_keg(keg_path) {
        Ok(linked_files) => Ok(linked_files),
        Err(Error::LinkConflict { conflicts }) if force => {
            for conflict in conflicts {
                match remove_path_any(&conflict.path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(Error::store("failed to replace existing linked file")(e));
                    }
                }
            }
            linker.link_keg(keg_path)
        }
        Err(e) => Err(e),
    }
}

struct FailedInstallGuard<'a> {
    linker: &'a Linker,
    cellar: &'a Cellar,
    name: &'a str,
    version: &'a str,
    keg_path: &'a Path,
    appimage_dir: &'a Path,
    unlink: bool,
    armed: bool,
}

impl<'a> FailedInstallGuard<'a> {
    fn new(
        linker: &'a Linker,
        cellar: &'a Cellar,
        name: &'a str,
        version: &'a str,
        keg_path: &'a Path,
        appimage_dir: &'a Path,
        unlink: bool,
    ) -> Self {
        Self {
            linker,
            cellar,
            name,
            version,
            keg_path,
            appimage_dir,
            unlink,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for FailedInstallGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            Installer::cleanup_failed_install(
                self.linker,
                self.cellar,
                self.name,
                self.version,
                self.keg_path,
                self.appimage_dir,
                self.unlink,
            );
        }
    }
}

fn stage_cask_binaries(
    extracted_root: &Path,
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<(), Error> {
    if cask.binaries.is_empty() {
        return Ok(());
    }

    let bin_dir = keg_path.join("bin");
    fs::create_dir_all(&bin_dir).map_err(Error::store("failed to create cask bin dir"))?;

    for binary in &cask.binaries {
        let source =
            resolve_cask_binary_source_path(extracted_root, keg_path, cask, &binary.source)?;
        if !source.exists() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "cask '{}' binary source '{}' not found",
                    cask.token, binary.source
                ),
            });
        }

        let target = bin_dir.join(&binary.target);
        if target.exists() {
            fs::remove_file(&target)
                .map_err(Error::store("failed to replace existing cask binary"))?;
        }

        fs::copy(&source, &target).map_err(|e| Error::StoreCorruption {
            message: format!("failed to stage cask binary '{}': {e}", binary.target),
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&target)
                .map_err(Error::store("failed to read staged cask binary metadata"))?
                .permissions();
            if perms.mode() & 0o111 == 0 {
                perms.set_mode(0o755);
                fs::set_permissions(&target, perms)
                    .map_err(Error::store("failed to make staged cask binary executable"))?;
            }
        }
    }

    Ok(())
}

fn stage_raw_cask_binary(
    blob_path: &Path,
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<(), Error> {
    if !cask.apps.is_empty()
        || !cask.fonts.is_empty()
        || !cask.pkgs.is_empty()
        || !cask.suites.is_empty()
        || !cask.generic_artifacts.is_empty()
        || !cask.app_images.is_empty()
        || cask.binaries.len() != 1
    {
        return Err(Error::InvalidArgument {
            message: format!(
                "cask '{}' has unsupported raw download layout; expected exactly 1 binary artifact and no app/font/pkg/suite/artifact/appimage artifacts",
                cask.token
            ),
        });
    }

    let binary = &cask.binaries[0];
    let bin_dir = keg_path.join("bin");
    fs::create_dir_all(&bin_dir).map_err(Error::store("failed to create cask bin dir"))?;

    let target = bin_dir.join(&binary.target);
    if target.exists() {
        fs::remove_file(&target).map_err(Error::store("failed to replace existing cask binary"))?;
    }

    fs::copy(blob_path, &target).map_err(|e| Error::StoreCorruption {
        message: format!("failed to stage cask binary '{}': {e}", binary.target),
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))
            .map_err(Error::store("failed to make staged cask binary executable"))?;
    }

    Ok(())
}

fn stage_raw_cask_pkg(
    blob_path: &Path,
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<(), Error> {
    if !cask.apps.is_empty()
        || !cask.fonts.is_empty()
        || !cask.binaries.is_empty()
        || !cask.suites.is_empty()
        || !cask.generic_artifacts.is_empty()
        || !cask.app_images.is_empty()
        || cask.pkgs.len() != 1
    {
        return Err(Error::InvalidArgument {
            message: format!(
                "cask '{}' has unsupported raw download layout; expected exactly 1 pkg artifact and no app/font/binary/suite/artifact/appimage artifacts",
                cask.token
            ),
        });
    }

    let pkg = &cask.pkgs[0];
    let pkg_dir = keg_path.join(CASK_PKGS_DIR);
    fs::create_dir_all(&pkg_dir).map_err(Error::store("failed to create cask pkg dir"))?;
    let target = pkg_dir.join(
        Path::new(&pkg.source)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("installer.pkg"),
    );
    fs::copy(blob_path, &target).map_err(Error::store("failed to stage raw cask pkg"))?;
    Ok(())
}

fn stage_raw_cask_appimage(
    blob_path: &Path,
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<(), Error> {
    if !cask.apps.is_empty()
        || !cask.fonts.is_empty()
        || !cask.binaries.is_empty()
        || !cask.pkgs.is_empty()
        || !cask.suites.is_empty()
        || !cask.generic_artifacts.is_empty()
        || cask.app_images.len() != 1
    {
        return Err(Error::InvalidArgument {
            message: format!(
                "cask '{}' has unsupported raw download layout; expected exactly 1 appimage artifact and no app/font/binary/pkg/suite/artifact artifacts",
                cask.token
            ),
        });
    }

    let app_image = &cask.app_images[0];
    let app_images_dir = keg_path.join(CASK_APP_IMAGES_DIR);
    fs::create_dir_all(&app_images_dir)
        .map_err(Error::store("failed to create cask appimage dir"))?;
    let target = app_images_dir.join(&app_image.target);
    fs::copy(blob_path, &target).map_err(Error::store("failed to stage raw cask appimage"))?;
    make_executable(&target)?;
    Ok(())
}

fn stage_cask_artifacts(
    extracted_root: &Path,
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<(), Error> {
    stage_cask_apps(extracted_root, keg_path, cask)?;
    stage_cask_fonts(extracted_root, keg_path, cask)?;
    stage_cask_pkgs(extracted_root, keg_path, cask)?;
    stage_cask_suites(extracted_root, keg_path, cask)?;
    stage_cask_generic_artifacts(extracted_root, keg_path, cask)?;
    stage_cask_app_images(extracted_root, keg_path, cask)?;
    stage_cask_binaries(extracted_root, keg_path, cask)?;
    Ok(())
}

fn is_dmg_cask(blob_path: &Path, cask: &crate::installer::cask::ResolvedCask) -> bool {
    blob_path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("dmg"))
        || cask
            .url
            .split(['?', '#'])
            .next()
            .is_some_and(|url_path| url_path.to_ascii_lowercase().ends_with(".dmg"))
}

fn is_raw_pkg_cask(blob_path: &Path, cask: &crate::installer::cask::ResolvedCask) -> bool {
    cask.pkgs.len() == 1
        && (blob_path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pkg"))
            || cask
                .url
                .split(['?', '#'])
                .next()
                .is_some_and(|url_path| url_path.to_ascii_lowercase().ends_with(".pkg")))
}

fn is_raw_appimage_cask(blob_path: &Path, cask: &crate::installer::cask::ResolvedCask) -> bool {
    cask.app_images.len() == 1
        && (blob_path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("appimage"))
            || cask
                .url
                .split(['?', '#'])
                .next()
                .is_some_and(|url_path| url_path.to_ascii_lowercase().ends_with(".appimage")))
}

#[cfg(target_os = "macos")]
fn ensure_dmg_store_entry(
    store: &Store,
    store_key: &str,
    dmg_path: &Path,
) -> Result<PathBuf, Error> {
    let entry_path = store.entry_path(store_key);
    if entry_path.exists() {
        return Ok(entry_path);
    }

    let store_dir = entry_path.parent().ok_or_else(|| Error::StoreCorruption {
        message: format!("store entry '{}' has no parent", entry_path.display()),
    })?;
    let extracted = tempfile::tempdir_in(store_dir)
        .map_err(Error::store("failed to create dmg store entry"))?;
    let mount = DmgMount::attach(dmg_path)?;

    for mount_point in &mount.mount_points {
        for entry in
            fs::read_dir(mount_point).map_err(Error::store("failed to read dmg mount point"))?
        {
            let entry = entry.map_err(Error::store("failed to read dmg entry"))?;
            copy_path_recursive(&entry.path(), &extracted.path().join(entry.file_name()))?;
        }
    }

    persist_temp_store_entry(extracted, &entry_path)
}

#[cfg(not(target_os = "macos"))]
fn ensure_dmg_store_entry(
    _store: &Store,
    _store_key: &str,
    _dmg_path: &Path,
) -> Result<PathBuf, Error> {
    Err(Error::InvalidArgument {
        message: "dmg casks are only supported on macOS".to_string(),
    })
}

fn ensure_raw_cask_store_entry(
    store: &Store,
    store_key: &str,
    blob_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<PathBuf, Error> {
    let entry_path = store.entry_path(store_key);
    if entry_path.exists() {
        return Ok(entry_path);
    }

    let store_dir = entry_path.parent().ok_or_else(|| Error::StoreCorruption {
        message: format!("store entry '{}' has no parent", entry_path.display()),
    })?;
    let staged = tempfile::tempdir_in(store_dir)
        .map_err(Error::store("failed to create cask store entry"))?;
    stage_raw_cask_binary(blob_path, staged.path(), cask)?;
    persist_temp_store_entry(staged, &entry_path)
}

fn ensure_raw_cask_pkg_entry(
    store: &Store,
    store_key: &str,
    blob_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<PathBuf, Error> {
    let entry_path = store.entry_path(store_key);
    if entry_path.exists() {
        return Ok(entry_path);
    }

    let store_dir = entry_path.parent().ok_or_else(|| Error::StoreCorruption {
        message: format!("store entry '{}' has no parent", entry_path.display()),
    })?;
    let staged = tempfile::tempdir_in(store_dir)
        .map_err(Error::store("failed to create cask pkg store entry"))?;
    stage_raw_cask_pkg(blob_path, staged.path(), cask)?;
    persist_temp_store_entry(staged, &entry_path)
}

fn ensure_raw_cask_appimage_entry(
    store: &Store,
    store_key: &str,
    blob_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<PathBuf, Error> {
    let entry_path = store.entry_path(store_key);
    if entry_path.exists() {
        return Ok(entry_path);
    }

    let store_dir = entry_path.parent().ok_or_else(|| Error::StoreCorruption {
        message: format!("store entry '{}' has no parent", entry_path.display()),
    })?;
    let staged = tempfile::tempdir_in(store_dir)
        .map_err(Error::store("failed to create cask appimage store entry"))?;
    stage_raw_cask_appimage(blob_path, staged.path(), cask)?;
    persist_temp_store_entry(staged, &entry_path)
}

fn persist_temp_store_entry(
    temp_dir: tempfile::TempDir,
    entry_path: &Path,
) -> Result<PathBuf, Error> {
    let tmp_path = temp_dir.keep();
    match fs::rename(&tmp_path, entry_path) {
        Ok(()) => Ok(entry_path.to_path_buf()),
        Err(_) if entry_path.exists() => {
            let _ = fs::remove_dir_all(&tmp_path);
            Ok(entry_path.to_path_buf())
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&tmp_path);
            Err(Error::StoreCorruption {
                message: format!("failed to rename store entry: {e}"),
            })
        }
    }
}

fn run_builtin_post_install(
    prefix: &Path,
    install_name: &str,
    keg_path: &Path,
) -> Result<(), Error> {
    match formula_token(install_name) {
        "ca-certificates" => install_ca_certificates_bundle(prefix, keg_path),
        _ => Ok(()),
    }
}

fn install_ca_certificates_bundle(prefix: &Path, keg_path: &Path) -> Result<(), Error> {
    let source = keg_path
        .join("share")
        .join("ca-certificates")
        .join("cacert.pem");
    if !source.exists() {
        return Ok(());
    }

    let target_dir = prefix.join("etc").join("ca-certificates");
    fs::create_dir_all(&target_dir)
        .map_err(Error::store("failed to create ca-certificates dir"))?;
    fs::copy(&source, target_dir.join("cert.pem"))
        .map_err(Error::store("failed to install ca-certificates bundle"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
struct DmgMount {
    mount_points: Vec<PathBuf>,
    _mount_root: tempfile::TempDir,
}

#[cfg(target_os = "macos")]
impl DmgMount {
    fn attach(dmg_path: &Path) -> Result<Self, Error> {
        let mount_root =
            tempfile::tempdir().map_err(Error::store("failed to create dmg mount dir"))?;
        let output = Command::new("hdiutil")
            .args(["attach", "-plist", "-nobrowse", "-readonly", "-mountrandom"])
            .arg(mount_root.path())
            .arg(dmg_path)
            .output()
            .map_err(Error::exec("failed to run hdiutil attach"))?;

        if !output.status.success() {
            return Err(Error::ExecutionError {
                message: format!(
                    "hdiutil attach failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        let plist = String::from_utf8_lossy(&output.stdout);
        let mount_points = parse_hdiutil_mount_points(&plist);
        if mount_points.is_empty() {
            return Err(Error::ExecutionError {
                message: "hdiutil attach returned no mount points".to_string(),
            });
        }

        Ok(Self {
            mount_points,
            _mount_root: mount_root,
        })
    }
}

#[cfg(target_os = "macos")]
impl Drop for DmgMount {
    fn drop(&mut self) {
        for mount_point in self.mount_points.iter().rev() {
            let _ = Command::new("hdiutil")
                .args(["detach", "-force"])
                .arg(mount_point)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn parse_hdiutil_mount_points(plist: &str) -> Vec<PathBuf> {
    let mut mount_points = Vec::new();
    let mut rest = plist;

    while let Some(key_idx) = rest.find("<key>mount-point</key>") {
        rest = &rest[key_idx + "<key>mount-point</key>".len()..];
        let Some(start_idx) = rest.find("<string>") else {
            break;
        };
        rest = &rest[start_idx + "<string>".len()..];
        let Some(end_idx) = rest.find("</string>") else {
            break;
        };
        mount_points.push(PathBuf::from(xml_unescape(&rest[..end_idx])));
        rest = &rest[end_idx + "</string>".len()..];
    }

    mount_points
}

#[cfg(any(target_os = "macos", test))]
fn xml_unescape(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn stage_cask_apps(
    extracted_root: &Path,
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<(), Error> {
    if cask.apps.is_empty() {
        return Ok(());
    }

    let apps_dir = keg_path.join(CASK_APPS_DIR);
    fs::create_dir_all(&apps_dir).map_err(Error::store("failed to create cask app dir"))?;

    for app in &cask.apps {
        let source = resolve_relative_cask_path(extracted_root, cask, &app.source, "app")?;
        if !source.exists() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "cask '{}' app source '{}' not found in archive",
                    cask.token, app.source
                ),
            });
        }

        let target = apps_dir.join(&app.target);
        if target.symlink_metadata().is_ok() {
            remove_path_any(&target)
                .map_err(Error::store("failed to replace existing cask app"))?;
        }

        copy_path_recursive(&source, &target)?;
    }

    Ok(())
}

fn stage_cask_fonts(
    extracted_root: &Path,
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<(), Error> {
    if cask.fonts.is_empty() {
        return Ok(());
    }

    let fonts_dir = keg_path.join(CASK_FONTS_DIR);
    fs::create_dir_all(&fonts_dir).map_err(Error::store("failed to create cask font dir"))?;

    for font in &cask.fonts {
        let source = resolve_relative_cask_path(extracted_root, cask, &font.source, "font")?;
        if !source.exists() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "cask '{}' font source '{}' not found in archive",
                    cask.token, font.source
                ),
            });
        }

        let target = fonts_dir.join(&font.target);
        if target.symlink_metadata().is_ok() {
            remove_path_any(&target)
                .map_err(Error::store("failed to replace existing cask font"))?;
        }

        copy_path_recursive(&source, &target)?;
    }

    Ok(())
}

fn stage_cask_pkgs(
    extracted_root: &Path,
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<(), Error> {
    if cask.pkgs.is_empty() {
        return Ok(());
    }

    let pkgs_dir = keg_path.join(CASK_PKGS_DIR);
    fs::create_dir_all(&pkgs_dir).map_err(Error::store("failed to create cask pkg dir"))?;

    for pkg in &cask.pkgs {
        let source = resolve_relative_cask_path(extracted_root, cask, &pkg.source, "pkg")?;
        if !source.exists() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "cask '{}' pkg source '{}' not found in archive",
                    cask.token, pkg.source
                ),
            });
        }

        let target = pkgs_dir.join(
            Path::new(&pkg.source)
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| Error::InvalidArgument {
                    message: format!("invalid cask pkg path '{}'", pkg.source),
                })?,
        );
        if target.symlink_metadata().is_ok() {
            remove_path_any(&target)
                .map_err(Error::store("failed to replace existing cask pkg"))?;
        }

        copy_path_recursive(&source, &target)?;
    }

    Ok(())
}

fn stage_cask_suites(
    extracted_root: &Path,
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<(), Error> {
    if cask.suites.is_empty() {
        return Ok(());
    }

    let apps_dir = keg_path.join(CASK_APPS_DIR);
    fs::create_dir_all(&apps_dir).map_err(Error::store("failed to create cask suite dir"))?;

    for suite in &cask.suites {
        let source = resolve_relative_cask_path(extracted_root, cask, &suite.source, "suite")?;
        if !source.exists() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "cask '{}' suite source '{}' not found in archive",
                    cask.token, suite.source
                ),
            });
        }

        let target = apps_dir.join(&suite.target);
        if target.symlink_metadata().is_ok() {
            remove_path_any(&target)
                .map_err(Error::store("failed to replace existing cask suite"))?;
        }

        copy_path_recursive(&source, &target)?;
    }

    Ok(())
}

fn stage_cask_generic_artifacts(
    extracted_root: &Path,
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<(), Error> {
    if cask.generic_artifacts.is_empty() {
        return Ok(());
    }

    let artifacts_dir = keg_path.join(CASK_GENERIC_ARTIFACTS_DIR);
    fs::create_dir_all(&artifacts_dir)
        .map_err(Error::store("failed to create cask generic artifact dir"))?;

    for artifact in &cask.generic_artifacts {
        let source =
            resolve_relative_cask_path(extracted_root, cask, &artifact.source, "artifact")?;
        if !source.exists() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "cask '{}' artifact source '{}' not found in archive",
                    cask.token, artifact.source
                ),
            });
        }

        let target = artifacts_dir.join(safe_staged_artifact_name(&artifact.target)?);
        if target.symlink_metadata().is_ok() {
            remove_path_any(&target).map_err(Error::store(
                "failed to replace existing cask generic artifact",
            ))?;
        }

        copy_path_recursive(&source, &target)?;
    }

    Ok(())
}

fn stage_cask_app_images(
    extracted_root: &Path,
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<(), Error> {
    if cask.app_images.is_empty() {
        return Ok(());
    }

    let app_images_dir = keg_path.join(CASK_APP_IMAGES_DIR);
    fs::create_dir_all(&app_images_dir)
        .map_err(Error::store("failed to create cask appimage dir"))?;

    for app_image in &cask.app_images {
        let source =
            resolve_relative_cask_path(extracted_root, cask, &app_image.source, "appimage")?;
        if !source.exists() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "cask '{}' appimage source '{}' not found in archive",
                    cask.token, app_image.source
                ),
            });
        }

        let target = app_images_dir.join(&app_image.target);
        if target.symlink_metadata().is_ok() {
            remove_path_any(&target)
                .map_err(Error::store("failed to replace existing cask appimage"))?;
        }

        copy_path_recursive(&source, &target)?;
        make_executable(&target)?;
    }

    Ok(())
}

fn resolve_cask_binary_source_path(
    extracted_root: &Path,
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
    source: &str,
) -> Result<PathBuf, Error> {
    if source.starts_with("$APPDIR") {
        let relative = source
            .strip_prefix("$APPDIR/")
            .or_else(|| source.strip_prefix("$APPDIR"))
            .unwrap_or(source);
        return resolve_relative_cask_path(&keg_path.join(CASK_APPS_DIR), cask, relative, "binary");
    }

    if Path::new(source).is_absolute() {
        for app in &cask.apps {
            let app_prefix = format!("/{}/", app.target);
            if let Some(idx) = source.find(&app_prefix) {
                let relative = &source[idx + 1..];
                return resolve_relative_cask_path(
                    &keg_path.join(CASK_APPS_DIR),
                    cask,
                    relative,
                    "binary",
                );
            }

            if app.source != app.target {
                let app_prefix = format!("/{}/", app.source);
                if let Some(idx) = source.find(&app_prefix) {
                    let suffix = &source[idx + app_prefix.len()..];
                    let relative = format!("{}/{}", app.target, suffix);
                    return resolve_relative_cask_path(
                        &keg_path.join(CASK_APPS_DIR),
                        cask,
                        &relative,
                        "binary",
                    );
                }
            }
        }
    }

    resolve_relative_cask_path(extracted_root, cask, source, "binary")
}

fn resolve_relative_cask_path(
    root: &Path,
    cask: &crate::installer::cask::ResolvedCask,
    source: &str,
    artifact_kind: &str,
) -> Result<PathBuf, Error> {
    let mut normalized = source.to_string();
    let caskroom_prefix = format!("$HOMEBREW_PREFIX/Caskroom/{}/{}/", cask.token, cask.version);
    if let Some(stripped) = normalized.strip_prefix(&caskroom_prefix) {
        normalized = stripped.to_string();
    }

    let source_path = Path::new(&normalized);
    if source_path.is_absolute() {
        return Err(Error::InvalidArgument {
            message: format!(
                "cask '{}' {artifact_kind} source '{}' must be a relative path",
                cask.token, source
            ),
        });
    }

    for component in source_path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(Error::InvalidArgument {
                message: format!(
                    "cask '{}' {artifact_kind} source '{}' cannot contain '..'",
                    cask.token, source
                ),
            });
        }
    }

    Ok(root.join(source_path))
}

fn copy_path_recursive(src: &Path, dst: &Path) -> Result<(), Error> {
    let metadata =
        fs::symlink_metadata(src).map_err(Error::store("failed to read source metadata"))?;

    if metadata.file_type().is_symlink() {
        let target = fs::read_link(src).map_err(Error::store("failed to read source symlink"))?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, dst)
            .map_err(Error::store("failed to create symlink"))?;
        #[cfg(not(unix))]
        fs::copy(src, dst).map_err(Error::store("failed to copy symlink as file"))?;
        return Ok(());
    }

    if metadata.is_dir() {
        fs::create_dir_all(dst).map_err(Error::store("failed to create target directory"))?;
        for entry in fs::read_dir(src).map_err(Error::store("failed to read source directory"))? {
            let entry = entry.map_err(Error::store("failed to read source directory entry"))?;
            copy_path_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
        return Ok(());
    }

    fs::copy(src, dst).map_err(Error::store("failed to copy file"))?;
    Ok(())
}

fn link_cask_apps(
    keg_path: &Path,
    app_dir: &Path,
    cask: &crate::installer::cask::ResolvedCask,
    force: bool,
) -> Result<(), Error> {
    if cask.apps.is_empty() && cask.suites.is_empty() {
        return Ok(());
    }

    fs::create_dir_all(app_dir).map_err(Error::store("failed to create app directory"))?;
    let staged_apps_dir = keg_path.join(CASK_APPS_DIR);

    for (target_name, kind) in cask
        .apps
        .iter()
        .map(|app| (app.target.as_str(), "app"))
        .chain(
            cask.suites
                .iter()
                .map(|suite| (suite.target.as_str(), "suite")),
        )
    {
        let source = staged_apps_dir.join(target_name);
        let target = app_dir.join(target_name);

        if !source.exists() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "cask '{}' {kind} '{}' was not staged correctly",
                    cask.token, target_name
                ),
            });
        }

        if target.symlink_metadata().is_ok() {
            if force {
                remove_path_any(&target)
                    .map_err(Error::store("failed to replace existing cask app"))?;
            } else {
                return Err(Error::LinkConflict {
                    conflicts: vec![zb_core::ConflictedLink {
                        path: target,
                        owned_by: None,
                    }],
                });
            }
        }

        move_cask_app_to_target(&source, &target)?;
    }

    Ok(())
}

fn link_cask_fonts(
    keg_path: &Path,
    font_dir: &Path,
    cask: &crate::installer::cask::ResolvedCask,
    force: bool,
) -> Result<(), Error> {
    if cask.fonts.is_empty() {
        return Ok(());
    }

    fs::create_dir_all(font_dir).map_err(Error::store("failed to create font directory"))?;
    let staged_fonts_dir = keg_path.join(CASK_FONTS_DIR);

    for font in &cask.fonts {
        let source = staged_fonts_dir.join(&font.target);
        let target = font_dir.join(&font.target);

        if !source.exists() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "cask '{}' font '{}' was not staged correctly",
                    cask.token, font.target
                ),
            });
        }

        if target.symlink_metadata().is_ok() {
            if force {
                remove_path_any(&target)
                    .map_err(Error::store("failed to replace existing cask font"))?;
            } else {
                return Err(Error::LinkConflict {
                    conflicts: vec![zb_core::ConflictedLink {
                        path: target,
                        owned_by: None,
                    }],
                });
            }
        }

        move_cask_artifact_to_target(&source, &target, "font")?;
    }

    Ok(())
}

fn link_cask_generic_artifacts(
    keg_path: &Path,
    prefix: &Path,
    cask: &crate::installer::cask::ResolvedCask,
    force: bool,
) -> Result<(), Error> {
    if cask.generic_artifacts.is_empty() {
        return Ok(());
    }

    let staged_artifacts_dir = keg_path.join(CASK_GENERIC_ARTIFACTS_DIR);
    for artifact in &cask.generic_artifacts {
        let source = staged_artifacts_dir.join(safe_staged_artifact_name(&artifact.target)?);
        let target = resolve_generic_artifact_target(prefix, &artifact.target)?;

        if !source.exists() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "cask '{}' artifact '{}' was not staged correctly",
                    cask.token, artifact.target
                ),
            });
        }

        if target.symlink_metadata().is_ok() {
            if force {
                remove_path_any(&target)
                    .map_err(Error::store("failed to replace existing cask artifact"))?;
            } else {
                return Err(Error::LinkConflict {
                    conflicts: vec![zb_core::ConflictedLink {
                        path: target,
                        owned_by: None,
                    }],
                });
            }
        }

        move_cask_artifact_to_target(&source, &target, "artifact")?;
    }

    Ok(())
}

fn link_cask_app_images(
    keg_path: &Path,
    appimage_dir: &Path,
    cask: &crate::installer::cask::ResolvedCask,
    force: bool,
) -> Result<(), Error> {
    if cask.app_images.is_empty() {
        return Ok(());
    }

    fs::create_dir_all(appimage_dir)
        .map_err(Error::store("failed to create appimage directory"))?;
    let staged_app_images_dir = keg_path.join(CASK_APP_IMAGES_DIR);

    for app_image in &cask.app_images {
        let source = staged_app_images_dir.join(&app_image.target);
        let target = appimage_dir.join(&app_image.target);

        if !source.exists() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "cask '{}' appimage '{}' was not staged correctly",
                    cask.token, app_image.target
                ),
            });
        }

        if target.symlink_metadata().is_ok() {
            if force {
                remove_path_any(&target)
                    .map_err(Error::store("failed to replace existing cask appimage"))?;
            } else {
                return Err(Error::LinkConflict {
                    conflicts: vec![zb_core::ConflictedLink {
                        path: target,
                        owned_by: None,
                    }],
                });
            }
        }

        #[cfg(unix)]
        std::os::unix::fs::symlink(&source, &target).map_err(|e| Error::StoreCorruption {
            message: format!("failed to link cask appimage '{}': {e}", app_image.target),
        })?;
        #[cfg(not(unix))]
        fs::copy(&source, &target).map_err(|e| Error::StoreCorruption {
            message: format!("failed to copy cask appimage '{}': {e}", app_image.target),
        })?;
        make_executable(&source)?;
    }

    Ok(())
}

fn move_cask_app_to_target(source: &Path, target: &Path) -> Result<(), Error> {
    move_cask_artifact_to_target(source, target, "app")
}

fn move_cask_artifact_to_target(source: &Path, target: &Path, kind: &str) -> Result<(), Error> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::StoreCorruption {
            message: format!("failed to create cask {kind} parent directory: {e}"),
        })?;
    }

    match fs::rename(source, target) {
        Ok(()) => {}
        Err(_) => {
            copy_path_recursive(source, target)?;
            remove_path_any(source).map_err(|e| Error::StoreCorruption {
                message: format!("failed to remove staged cask {kind} after copy: {e}"),
            })?;
        }
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(target, source).map_err(|e| Error::StoreCorruption {
        message: format!("failed to create staged cask {kind} symlink: {e}"),
    })?;

    Ok(())
}

fn resolve_generic_artifact_target(prefix: &Path, target: &str) -> Result<PathBuf, Error> {
    if let Some(relative) = target.strip_prefix("$HOMEBREW_PREFIX/") {
        return Ok(prefix.join(relative));
    }

    if target == "$HOMEBREW_PREFIX" {
        return Ok(prefix.to_path_buf());
    }

    let target_path = Path::new(target);
    if target_path.is_absolute() {
        return Err(Error::InvalidArgument {
            message: format!(
                "generic cask artifact target '{target}' is outside the zerobrew prefix"
            ),
        });
    }

    for component in target_path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(Error::InvalidArgument {
                message: format!("generic cask artifact target '{target}' cannot contain '..'"),
            });
        }
    }

    Ok(prefix.join(target_path))
}

fn safe_staged_artifact_name(target: &str) -> Result<String, Error> {
    let normalized = target
        .trim_start_matches("$HOMEBREW_PREFIX/")
        .trim_start_matches('/');
    if normalized.is_empty() || normalized.contains("..") {
        return Err(Error::InvalidArgument {
            message: format!("invalid cask artifact target '{target}'"),
        });
    }

    Ok(normalized.replace('/', "__"))
}

fn make_executable(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)
            .map_err(Error::store("failed to read cask executable metadata"))?
            .permissions();
        if perms.mode() & 0o111 == 0 {
            perms.set_mode(0o755);
            fs::set_permissions(path, perms)
                .map_err(Error::store("failed to make cask artifact executable"))?;
        }
    }

    Ok(())
}

fn run_cask_pkgs(
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<(), Error> {
    if cask.pkgs.is_empty() {
        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = keg_path;
        Err(Error::InvalidArgument {
            message: format!("pkg cask '{}' can only be installed on macOS", cask.token),
        })
    }

    #[cfg(target_os = "macos")]
    {
        for pkg in &cask.pkgs {
            let pkg_path = keg_path.join(CASK_PKGS_DIR).join(
                Path::new(&pkg.source)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| Error::InvalidArgument {
                        message: format!("invalid cask pkg path '{}'", pkg.source),
                    })?,
            );
            let status = Command::new("/usr/bin/sudo")
                .arg("/usr/sbin/installer")
                .args(["-pkg", &pkg_path.to_string_lossy(), "-target", "/"])
                .status()
                .map_err(Error::exec("failed to run cask pkg installer"))?;
            if !status.success() {
                return Err(Error::ExecutionError {
                    message: format!("pkg installer failed for '{}'", pkg_path.display()),
                });
            }
        }
        Ok(())
    }
}

fn run_cask_installers(
    keg_path: &Path,
    prefix: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> Result<(), Error> {
    for installer in &cask.installers {
        match &installer.kind {
            CaskInstallerKind::Manual { path } => {
                warn!(
                    cask = %cask.token,
                    path = %keg_path.join(path).display(),
                    "cask requires manual installer completion"
                );
            }
            CaskInstallerKind::Script { executable, args } => {
                let executable_path =
                    resolve_relative_cask_path(keg_path, cask, executable, "installer")?;
                if !executable_path.exists() {
                    return Err(Error::InvalidArgument {
                        message: format!(
                            "cask '{}' installer executable '{}' not found",
                            cask.token, executable
                        ),
                    });
                }
                let rendered_args: Vec<String> = args
                    .iter()
                    .map(|arg| render_cask_placeholder_arg(arg, prefix, keg_path, cask))
                    .collect();
                let status = Command::new(&executable_path)
                    .args(&rendered_args)
                    .current_dir(keg_path)
                    .status()
                    .map_err(Error::exec("failed to run cask installer script"))?;
                if !status.success() {
                    return Err(Error::ExecutionError {
                        message: format!(
                            "installer script failed for cask '{}': {}",
                            cask.token,
                            executable_path.display()
                        ),
                    });
                }
            }
        }
    }

    Ok(())
}

fn render_cask_placeholder_arg(
    arg: &str,
    prefix: &Path,
    keg_path: &Path,
    cask: &crate::installer::cask::ResolvedCask,
) -> String {
    arg.replace(
        &format!("$HOMEBREW_PREFIX/Caskroom/{}/{}", cask.token, cask.version),
        &keg_path.to_string_lossy(),
    )
    .replace("$HOMEBREW_PREFIX", &prefix.to_string_lossy())
    .replace("$HOMEBREW_CELLAR", &prefix.join("Cellar").to_string_lossy())
}

pub(super) fn uninstall_cask_apps(keg_path: &Path) -> Result<(), Error> {
    uninstall_moved_cask_artifacts(&keg_path.join(CASK_APPS_DIR), "app")
}

pub(super) fn uninstall_cask_fonts(keg_path: &Path) -> Result<(), Error> {
    uninstall_moved_cask_artifacts(&keg_path.join(CASK_FONTS_DIR), "font")
}

pub(super) fn uninstall_cask_generic_artifacts(keg_path: &Path) -> Result<(), Error> {
    uninstall_moved_cask_artifacts(&keg_path.join(CASK_GENERIC_ARTIFACTS_DIR), "artifact")
}

pub(super) fn uninstall_cask_app_images(keg_path: &Path, appimage_dir: &Path) -> Result<(), Error> {
    let staged_app_images_dir = keg_path.join(CASK_APP_IMAGES_DIR);
    if !staged_app_images_dir.exists() || !appimage_dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(appimage_dir).map_err(Error::store("failed to read appimage dir"))? {
        let entry = entry.map_err(Error::store("failed to read appimage dir entry"))?;
        let path = entry.path();
        let Ok(target) = fs::read_link(&path) else {
            continue;
        };
        let resolved = if target.is_relative() {
            path.parent().unwrap_or(Path::new("")).join(target)
        } else {
            target
        };
        if resolved.starts_with(&staged_app_images_dir) {
            fs::remove_file(&path).map_err(Error::store("failed to remove appimage symlink"))?;
        }
    }

    Ok(())
}

fn uninstall_moved_cask_artifacts(artifact_dir: &Path, kind: &str) -> Result<(), Error> {
    if !artifact_dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(artifact_dir).map_err(|e| Error::StoreCorruption {
        message: format!("failed to read cask {kind} dir: {e}"),
    })? {
        let entry = entry.map_err(|e| Error::StoreCorruption {
            message: format!("failed to read cask {kind} entry: {e}"),
        })?;
        let staged_path = entry.path();
        if !staged_path.is_symlink() {
            continue;
        }

        let target = resolve_staged_cask_target(&staged_path)?;
        if target.exists() {
            remove_path_any(&target).map_err(|e| Error::StoreCorruption {
                message: format!("failed to remove installed cask {kind}: {e}"),
            })?;
        }
    }

    Ok(())
}

fn resolve_staged_cask_target(staged_path: &Path) -> Result<PathBuf, Error> {
    let target = fs::read_link(staged_path).map_err(Error::store("failed to read app symlink"))?;
    Ok(if target.is_relative() {
        staged_path.parent().unwrap_or(Path::new("")).join(target)
    } else {
        target
    })
}

fn remove_path_any(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(path)
    } else {
        fs::remove_dir_all(path)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use crate::cellar::Cellar;
    use crate::storage::db::Database;

    use super::*;

    #[test]
    fn dependency_cellar_path_uses_formula_token_for_tap_name() {
        let tmp = TempDir::new().unwrap();
        let cellar = Cellar::new(tmp.path()).unwrap();
        let path = dependency_cellar_path(&cellar, "hashicorp/tap/terraform", "1.10.0");

        assert!(path.ends_with("cellar/terraform/1.10.0"));
    }

    #[test]
    fn dependency_cellar_path_keeps_core_formula_name() {
        let tmp = TempDir::new().unwrap();
        let cellar = Cellar::new(tmp.path()).unwrap();
        let path = dependency_cellar_path(&cellar, "openssl@3", "3.3.2");

        assert!(path.ends_with("cellar/openssl@3/3.3.2"));
    }

    #[test]
    fn dependency_cellar_path_uses_name_from_db_record() {
        let tmp = TempDir::new().unwrap();
        let cellar = Cellar::new(tmp.path()).unwrap();

        let db_path = tmp.path().join("zb.sqlite3");
        let mut db = Database::open(&db_path).unwrap();
        let tx = db.transaction().unwrap();
        tx.record_install("hashicorp/tap/terraform", "1.10.0", "store-key")
            .unwrap();
        tx.commit().unwrap();

        let keg = db.get_installed("hashicorp/tap/terraform").unwrap();
        let path = dependency_cellar_path(&cellar, &keg.name, &keg.version);

        assert!(path.ends_with("cellar/terraform/1.10.0"));
    }

    #[test]
    fn stage_raw_cask_binary_copies_and_marks_executable() {
        let tmp = TempDir::new().unwrap();
        let blob_path = tmp.path().join("claude");
        fs::write(&blob_path, b"#!/bin/sh\necho hello").unwrap();

        let keg_path = tmp.path().join("keg");
        let cask = crate::installer::cask::ResolvedCask {
            install_name: "cask:claude-code".to_string(),
            token: "claude-code".to_string(),
            version: "1.0.0".to_string(),
            url: "https://example.com/claude".to_string(),
            sha256: "aaa".to_string(),
            binaries: vec![crate::installer::cask::CaskBinary {
                source: "claude".to_string(),
                target: "claude".to_string(),
            }],
            apps: vec![],
            fonts: vec![],
            pkgs: vec![],
            suites: vec![],

            generic_artifacts: vec![],

            app_images: vec![],

            installers: vec![],
            stage_only: false,

            depends_on_formulas: vec![],

            depends_on_casks: vec![],
        };

        stage_raw_cask_binary(&blob_path, &keg_path, &cask).unwrap();

        let target = keg_path.join("bin/claude");
        assert!(target.exists());
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "#!/bin/sh\necho hello"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(mode & 0o755, 0o755);
        }
    }

    #[test]
    fn stage_raw_cask_binary_rejects_multiple_binaries() {
        let tmp = TempDir::new().unwrap();
        let blob_path = tmp.path().join("blob");
        fs::write(&blob_path, b"data").unwrap();

        let keg_path = tmp.path().join("keg");
        let cask = crate::installer::cask::ResolvedCask {
            install_name: "cask:multi".to_string(),
            token: "multi".to_string(),
            version: "1.0.0".to_string(),
            url: "https://example.com/multi".to_string(),
            sha256: "bbb".to_string(),
            binaries: vec![
                crate::installer::cask::CaskBinary {
                    source: "a".to_string(),
                    target: "a".to_string(),
                },
                crate::installer::cask::CaskBinary {
                    source: "b".to_string(),
                    target: "b".to_string(),
                },
            ],
            apps: vec![],
            fonts: vec![],
            pkgs: vec![],
            suites: vec![],

            generic_artifacts: vec![],

            app_images: vec![],

            installers: vec![],
            stage_only: false,

            depends_on_formulas: vec![],

            depends_on_casks: vec![],
        };

        let err = stage_raw_cask_binary(&blob_path, &keg_path, &cask).unwrap_err();
        assert!(err.to_string().contains("unsupported raw download layout"));
    }

    #[test]
    fn stage_cask_artifacts_copies_pkg_artifacts() {
        let tmp = TempDir::new().unwrap();
        let extracted_root = tmp.path().join("extract");
        fs::create_dir_all(&extracted_root).unwrap();
        fs::write(extracted_root.join("Install Test.pkg"), b"pkg").unwrap();

        let keg_path = tmp.path().join("keg");
        let cask = crate::installer::cask::ResolvedCask {
            install_name: "cask:pkg-test".to_string(),
            token: "pkg-test".to_string(),
            version: "1.0.0".to_string(),
            url: "https://example.com/pkg.zip".to_string(),
            sha256: "pkg".to_string(),
            binaries: vec![],
            apps: vec![],
            fonts: vec![],
            pkgs: vec![crate::installer::cask::CaskPkg {
                source: "Install Test.pkg".to_string(),
            }],
            suites: vec![],

            generic_artifacts: vec![],

            app_images: vec![],

            installers: vec![],
            stage_only: false,

            depends_on_formulas: vec![],

            depends_on_casks: vec![],
        };

        stage_cask_artifacts(&extracted_root, &keg_path, &cask).unwrap();

        assert_eq!(
            fs::read(keg_path.join("Pkgs/Install Test.pkg")).unwrap(),
            b"pkg"
        );
    }

    #[test]
    fn stage_raw_cask_pkg_copies_pkg_artifact() {
        let tmp = TempDir::new().unwrap();
        let blob_path = tmp.path().join("installer.pkg");
        fs::write(&blob_path, b"pkg").unwrap();

        let keg_path = tmp.path().join("keg");
        let cask = crate::installer::cask::ResolvedCask {
            install_name: "cask:pkg-test".to_string(),
            token: "pkg-test".to_string(),
            version: "1.0.0".to_string(),
            url: "https://example.com/installer.pkg".to_string(),
            sha256: "pkg".to_string(),
            binaries: vec![],
            apps: vec![],
            fonts: vec![],
            pkgs: vec![crate::installer::cask::CaskPkg {
                source: "installer.pkg".to_string(),
            }],
            suites: vec![],

            generic_artifacts: vec![],

            app_images: vec![],

            installers: vec![],
            stage_only: false,

            depends_on_formulas: vec![],

            depends_on_casks: vec![],
        };

        stage_raw_cask_pkg(&blob_path, &keg_path, &cask).unwrap();

        assert_eq!(
            fs::read(keg_path.join("Pkgs/installer.pkg")).unwrap(),
            b"pkg"
        );
    }

    #[test]
    fn stage_cask_artifacts_copies_suite_generic_and_appimage() {
        let tmp = TempDir::new().unwrap();
        let extracted_root = tmp.path().join("extract");
        fs::create_dir_all(extracted_root.join("Suite")).unwrap();
        fs::write(extracted_root.join("Suite/tool"), b"suite").unwrap();
        fs::create_dir_all(extracted_root.join("config")).unwrap();
        fs::write(extracted_root.join("config/example.conf"), b"config").unwrap();
        fs::write(extracted_root.join("Demo.AppImage"), b"appimage").unwrap();

        let keg_path = tmp.path().join("keg");
        let cask = crate::installer::cask::ResolvedCask {
            install_name: "cask:mixed-artifacts".to_string(),
            token: "mixed-artifacts".to_string(),
            version: "1.0.0".to_string(),
            url: "https://example.com/mixed.zip".to_string(),
            sha256: "mixed".to_string(),
            binaries: vec![],
            apps: vec![],
            fonts: vec![],
            pkgs: vec![],
            suites: vec![crate::installer::cask::CaskSuite {
                source: "Suite".to_string(),
                target: "Suite".to_string(),
            }],
            generic_artifacts: vec![crate::installer::cask::CaskGenericArtifact {
                source: "config/example.conf".to_string(),
                target: "etc/example.conf".to_string(),
            }],
            app_images: vec![crate::installer::cask::CaskAppImage {
                source: "Demo.AppImage".to_string(),
                target: "Demo.AppImage".to_string(),
            }],
            installers: vec![],
            stage_only: false,

            depends_on_formulas: vec![],

            depends_on_casks: vec![],
        };

        stage_cask_artifacts(&extracted_root, &keg_path, &cask).unwrap();

        assert_eq!(
            fs::read(keg_path.join("Applications/Suite/tool")).unwrap(),
            b"suite"
        );
        assert_eq!(
            fs::read(keg_path.join("Artifacts/etc__example.conf")).unwrap(),
            b"config"
        );
        assert_eq!(
            fs::read(keg_path.join("AppImages/Demo.AppImage")).unwrap(),
            b"appimage"
        );
    }

    #[test]
    fn link_cask_artifacts_links_suite_generic_and_appimage() {
        let tmp = TempDir::new().unwrap();
        let prefix = tmp.path().join("prefix");
        let app_dir = tmp.path().join("Applications");
        let appimage_dir = tmp.path().join("AppImages");
        let keg_path = tmp.path().join("keg");
        fs::create_dir_all(keg_path.join("Applications/Suite")).unwrap();
        fs::write(keg_path.join("Applications/Suite/tool"), b"suite").unwrap();
        fs::create_dir_all(keg_path.join("Artifacts")).unwrap();
        fs::write(keg_path.join("Artifacts/etc__example.conf"), b"config").unwrap();
        fs::create_dir_all(keg_path.join("AppImages")).unwrap();
        fs::write(keg_path.join("AppImages/Demo.AppImage"), b"appimage").unwrap();

        let cask = crate::installer::cask::ResolvedCask {
            install_name: "cask:mixed-artifacts".to_string(),
            token: "mixed-artifacts".to_string(),
            version: "1.0.0".to_string(),
            url: "https://example.com/mixed.zip".to_string(),
            sha256: "mixed".to_string(),
            binaries: vec![],
            apps: vec![],
            fonts: vec![],
            pkgs: vec![],
            suites: vec![crate::installer::cask::CaskSuite {
                source: "Suite".to_string(),
                target: "Suite".to_string(),
            }],
            generic_artifacts: vec![crate::installer::cask::CaskGenericArtifact {
                source: "config/example.conf".to_string(),
                target: "etc/example.conf".to_string(),
            }],
            app_images: vec![crate::installer::cask::CaskAppImage {
                source: "Demo.AppImage".to_string(),
                target: "Demo.AppImage".to_string(),
            }],
            installers: vec![],
            stage_only: false,

            depends_on_formulas: vec![],

            depends_on_casks: vec![],
        };

        link_cask_apps(&keg_path, &app_dir, &cask, false).unwrap();
        link_cask_generic_artifacts(&keg_path, &prefix, &cask, false).unwrap();
        link_cask_app_images(&keg_path, &appimage_dir, &cask, false).unwrap();

        assert_eq!(fs::read(app_dir.join("Suite/tool")).unwrap(), b"suite");
        assert_eq!(
            fs::read(prefix.join("etc/example.conf")).unwrap(),
            b"config"
        );
        assert!(appimage_dir.join("Demo.AppImage").exists());
        uninstall_cask_apps(&keg_path).unwrap();
        uninstall_cask_generic_artifacts(&keg_path).unwrap();
        uninstall_cask_app_images(&keg_path, &appimage_dir).unwrap();
        assert!(!app_dir.join("Suite").exists());
        assert!(!prefix.join("etc/example.conf").exists());
        assert!(!appimage_dir.join("Demo.AppImage").exists());
    }

    #[test]
    fn stage_only_cask_copies_extracted_tree() {
        let tmp = TempDir::new().unwrap();
        let extracted = tmp.path().join("extract");
        fs::create_dir_all(extracted.join("sqlcl/bin")).unwrap();
        fs::write(extracted.join("sqlcl/bin/sql"), b"sql").unwrap();
        let keg_path = tmp.path().join("keg");

        copy_path_recursive(&extracted, &keg_path).unwrap();

        assert_eq!(fs::read(keg_path.join("sqlcl/bin/sql")).unwrap(), b"sql");
    }

    #[test]
    fn run_cask_installers_executes_script_with_placeholders() {
        let tmp = TempDir::new().unwrap();
        let prefix = tmp.path().join("prefix");
        let keg_path = tmp.path().join("keg");
        fs::create_dir_all(&keg_path).unwrap();
        let script_path = keg_path.join("install.sh");
        fs::write(
            &script_path,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > args.txt\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let cask = crate::installer::cask::ResolvedCask {
            install_name: "cask:installer-test".to_string(),
            token: "installer-test".to_string(),
            version: "1.0.0".to_string(),
            url: "https://example.com/installer.zip".to_string(),
            sha256: "installer".to_string(),
            binaries: vec![],
            apps: vec![],
            fonts: vec![],
            pkgs: vec![],
            suites: vec![],

            generic_artifacts: vec![],

            app_images: vec![],

            installers: vec![crate::installer::cask::CaskInstaller {
                kind: crate::installer::cask::CaskInstallerKind::Script {
                    executable: "install.sh".to_string(),
                    args: vec![
                        "$HOMEBREW_PREFIX".to_string(),
                        "$HOMEBREW_PREFIX/Caskroom/installer-test/1.0.0".to_string(),
                    ],
                },
            }],
            stage_only: false,

            depends_on_formulas: vec![],

            depends_on_casks: vec![],
        };

        run_cask_installers(&keg_path, &prefix, &cask).unwrap();

        assert_eq!(
            fs::read_to_string(keg_path.join("args.txt")).unwrap(),
            format!("{}\n{}\n", prefix.display(), keg_path.display())
        );
    }

    #[test]
    fn stage_cask_artifacts_supports_appdir_binary_sources() {
        let tmp = TempDir::new().unwrap();
        let extracted_root = tmp.path().join("extract");
        let app_binary_dir =
            extracted_root.join("Visual Studio Code.app/Contents/Resources/app/bin");
        fs::create_dir_all(&app_binary_dir).unwrap();
        fs::write(app_binary_dir.join("code"), b"#!/bin/sh\necho code").unwrap();
        fs::write(
            extracted_root.join("Visual Studio Code.app/Contents/Info.plist"),
            b"plist",
        )
        .unwrap();

        let keg_path = tmp.path().join("keg");
        let cask = crate::installer::cask::ResolvedCask {
            install_name: "cask:visual-studio-code".to_string(),
            token: "visual-studio-code".to_string(),
            version: "1.0.0".to_string(),
            url: "https://example.com/vscode.zip".to_string(),
            sha256: "ccc".to_string(),
            binaries: vec![crate::installer::cask::CaskBinary {
                source: "$APPDIR/Visual Studio Code.app/Contents/Resources/app/bin/code"
                    .to_string(),
                target: "code".to_string(),
            }],
            apps: vec![crate::installer::cask::CaskApp {
                source: "Visual Studio Code.app".to_string(),
                target: "Visual Studio Code.app".to_string(),
            }],
            fonts: vec![],
            pkgs: vec![],
            suites: vec![],

            generic_artifacts: vec![],

            app_images: vec![],

            installers: vec![],
            stage_only: false,

            depends_on_formulas: vec![],

            depends_on_casks: vec![],
        };

        stage_cask_artifacts(&extracted_root, &keg_path, &cask).unwrap();

        assert_eq!(
            fs::read_to_string(keg_path.join("bin/code")).unwrap(),
            "#!/bin/sh\necho code"
        );
        assert!(
            keg_path
                .join("Applications/Visual Studio Code.app/Contents/Info.plist")
                .exists()
        );
    }

    #[test]
    fn stage_cask_artifacts_supports_resolved_appdir_binary_sources() {
        let tmp = TempDir::new().unwrap();
        let extracted_root = tmp.path().join("extract");
        let app_binary_dir = extracted_root.join("OmniWM.app/Contents/MacOS");
        fs::create_dir_all(&app_binary_dir).unwrap();
        fs::write(app_binary_dir.join("omniwmctl"), b"#!/bin/sh\necho omni").unwrap();
        fs::write(
            extracted_root.join("OmniWM.app/Contents/Info.plist"),
            b"plist",
        )
        .unwrap();

        let keg_path = tmp.path().join("keg");
        let cask = crate::installer::cask::ResolvedCask {
            install_name: "cask:omniwm".to_string(),
            token: "omniwm".to_string(),
            version: "1.0.0".to_string(),
            url: "https://example.com/omniwm.zip".to_string(),
            sha256: "fff".to_string(),
            binaries: vec![crate::installer::cask::CaskBinary {
                source: "/Applications/OmniWM.app/Contents/MacOS/omniwmctl".to_string(),
                target: "omniwmctl".to_string(),
            }],
            apps: vec![crate::installer::cask::CaskApp {
                source: "OmniWM.app".to_string(),
                target: "OmniWM.app".to_string(),
            }],
            fonts: vec![],
            pkgs: vec![],
            suites: vec![],

            generic_artifacts: vec![],

            app_images: vec![],

            installers: vec![],
            stage_only: false,

            depends_on_formulas: vec![],

            depends_on_casks: vec![],
        };

        stage_cask_artifacts(&extracted_root, &keg_path, &cask).unwrap();

        assert_eq!(
            fs::read_to_string(keg_path.join("bin/omniwmctl")).unwrap(),
            "#!/bin/sh\necho omni"
        );
    }

    #[test]
    fn stage_cask_artifacts_resolves_absolute_binary_sources_for_renamed_apps() {
        let tmp = TempDir::new().unwrap();
        let extracted_root = tmp.path().join("extract");
        let app_binary_dir = extracted_root.join("Foo.app/Contents/MacOS");
        fs::create_dir_all(&app_binary_dir).unwrap();
        fs::write(app_binary_dir.join("fooctl"), b"#!/bin/sh\necho foo").unwrap();
        fs::write(extracted_root.join("Foo.app/Contents/Info.plist"), b"plist").unwrap();

        let keg_path = tmp.path().join("keg");
        let cask = crate::installer::cask::ResolvedCask {
            install_name: "cask:foo".to_string(),
            token: "foo".to_string(),
            version: "1.0.0".to_string(),
            url: "https://example.com/foo.zip".to_string(),
            sha256: "fff".to_string(),
            binaries: vec![crate::installer::cask::CaskBinary {
                source: "/Applications/Foo.app/Contents/MacOS/fooctl".to_string(),
                target: "fooctl".to_string(),
            }],
            apps: vec![crate::installer::cask::CaskApp {
                source: "Foo.app".to_string(),
                target: "Bar.app".to_string(),
            }],
            fonts: vec![],
            pkgs: vec![],
            suites: vec![],
            generic_artifacts: vec![],
            app_images: vec![],
            installers: vec![],
            stage_only: false,
            depends_on_formulas: vec![],
            depends_on_casks: vec![],
        };

        stage_cask_artifacts(&extracted_root, &keg_path, &cask).unwrap();

        assert_eq!(
            fs::read_to_string(keg_path.join("bin/fooctl")).unwrap(),
            "#!/bin/sh\necho foo"
        );
        assert!(
            keg_path
                .join("Applications/Bar.app/Contents/Info.plist")
                .exists()
        );
    }

    #[test]
    fn parses_hdiutil_mount_points() {
        let plist = r#"
        <plist version="1.0">
        <dict>
          <key>system-entities</key>
          <array>
            <dict>
              <key>mount-point</key>
              <string>/private/tmp/homebrew-dmg/Test &amp; App</string>
            </dict>
          </array>
        </dict>
        </plist>
        "#;

        let mount_points = parse_hdiutil_mount_points(plist);
        assert_eq!(mount_points.len(), 1);
        assert_eq!(
            mount_points[0],
            PathBuf::from("/private/tmp/homebrew-dmg/Test & App")
        );
    }

    #[test]
    fn ca_certificates_post_install_writes_prefix_bundle() {
        let tmp = TempDir::new().unwrap();
        let prefix = tmp.path().join("prefix");
        let keg_path = tmp.path().join("Cellar/ca-certificates/2026-03-19");
        let source = keg_path.join("share/ca-certificates/cacert.pem");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"certs").unwrap();

        run_builtin_post_install(&prefix, "ca-certificates", &keg_path).unwrap();

        assert_eq!(
            fs::read(prefix.join("etc/ca-certificates/cert.pem")).unwrap(),
            b"certs"
        );
    }

    #[test]
    fn link_cask_apps_moves_app_and_leaves_keg_symlink() {
        let tmp = TempDir::new().unwrap();
        let keg_path = tmp.path().join("keg");
        let app_dir = tmp.path().join("Applications");
        let staged_app = keg_path.join("Applications/Test.app");
        fs::create_dir_all(staged_app.join("Contents")).unwrap();
        fs::write(staged_app.join("Contents/Info.plist"), b"plist").unwrap();

        let cask = crate::installer::cask::ResolvedCask {
            install_name: "cask:test".to_string(),
            token: "test".to_string(),
            version: "1.0.0".to_string(),
            url: "https://example.com/test.zip".to_string(),
            sha256: "ddd".to_string(),
            binaries: vec![],
            apps: vec![crate::installer::cask::CaskApp {
                source: "Test.app".to_string(),
                target: "Test.app".to_string(),
            }],
            fonts: vec![],
            pkgs: vec![],
            suites: vec![],

            generic_artifacts: vec![],

            app_images: vec![],

            installers: vec![],
            stage_only: false,

            depends_on_formulas: vec![],

            depends_on_casks: vec![],
        };

        link_cask_apps(&keg_path, &app_dir, &cask, false).unwrap();

        assert!(app_dir.join("Test.app/Contents/Info.plist").exists());
        assert!(staged_app.is_symlink());
        assert_eq!(
            resolve_staged_cask_target(&staged_app).unwrap(),
            app_dir.join("Test.app")
        );

        uninstall_cask_apps(&keg_path).unwrap();
        assert!(!app_dir.join("Test.app").exists());
    }

    #[test]
    fn link_cask_fonts_moves_font_and_leaves_keg_symlink() {
        let tmp = TempDir::new().unwrap();
        let keg_path = tmp.path().join("keg");
        let font_dir = tmp.path().join("Fonts");
        let staged_font = keg_path.join("Fonts/Test-Regular.otf");
        fs::create_dir_all(staged_font.parent().unwrap()).unwrap();
        fs::write(&staged_font, b"font").unwrap();

        let cask = crate::installer::cask::ResolvedCask {
            install_name: "cask:font-test".to_string(),
            token: "font-test".to_string(),
            version: "1.0.0".to_string(),
            url: "https://example.com/font.zip".to_string(),
            sha256: "eee".to_string(),
            binaries: vec![],
            apps: vec![],
            fonts: vec![crate::installer::cask::CaskFont {
                source: "Test-Regular.otf".to_string(),
                target: "Test-Regular.otf".to_string(),
            }],
            pkgs: vec![],
            suites: vec![],

            generic_artifacts: vec![],

            app_images: vec![],

            installers: vec![],
            stage_only: false,

            depends_on_formulas: vec![],

            depends_on_casks: vec![],
        };

        link_cask_fonts(&keg_path, &font_dir, &cask, false).unwrap();

        assert_eq!(
            fs::read_to_string(font_dir.join("Test-Regular.otf")).unwrap(),
            "font"
        );
        assert!(staged_font.is_symlink());

        uninstall_cask_fonts(&keg_path).unwrap();
        assert!(!font_dir.join("Test-Regular.otf").exists());
    }

    #[test]
    fn force_link_keg_replaces_existing_conflicts() {
        let tmp = TempDir::new().unwrap();
        let prefix = tmp.path().join("prefix");
        let linker = Linker::new(&prefix).unwrap();
        let cellar = Cellar::new(&prefix).unwrap();
        let keg_path = cellar.keg_path("cask:test", "1.0.0");

        let staged_binary = keg_path.join("bin/test-tool");
        fs::create_dir_all(staged_binary.parent().unwrap()).unwrap();
        fs::write(&staged_binary, b"#!/bin/sh\necho staged\n").unwrap();

        let conflict = prefix.join("bin/test-tool");
        fs::write(&conflict, b"existing").unwrap();

        let result = link_keg_with_force(&linker, &keg_path, false);
        assert!(matches!(result, Err(Error::LinkConflict { .. })));
        assert_eq!(fs::read_to_string(&conflict).unwrap(), "existing");

        let linked_files = link_keg_with_force(&linker, &keg_path, true).unwrap();

        assert_eq!(linked_files.len(), 1);
        assert!(conflict.is_symlink());
        assert_eq!(fs::read_link(&conflict).unwrap(), staged_binary);
    }

    #[test]
    fn cleanup_failed_install_removes_moved_cask_apps_fonts_and_appimages() {
        let tmp = TempDir::new().unwrap();
        let prefix = tmp.path().join("prefix");
        let linker = Linker::new(&prefix).unwrap();
        let cellar = Cellar::new(&prefix).unwrap();
        let keg_path = cellar.keg_path("cask:test", "1.0.0");
        let appimage_dir = tmp.path().join("AppImages");

        let app_target = tmp.path().join("Applications/Test.app");
        fs::create_dir_all(app_target.join("Contents")).unwrap();
        fs::write(app_target.join("Contents/Info.plist"), b"plist").unwrap();
        let staged_app = keg_path.join("Applications/Test.app");
        fs::create_dir_all(staged_app.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&app_target, &staged_app).unwrap();

        let font_target = tmp.path().join("Fonts/Test-Regular.otf");
        fs::create_dir_all(font_target.parent().unwrap()).unwrap();
        fs::write(&font_target, b"font").unwrap();
        let staged_font = keg_path.join("Fonts/Test-Regular.otf");
        fs::create_dir_all(staged_font.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&font_target, &staged_font).unwrap();

        fs::create_dir_all(&appimage_dir).unwrap();
        let staged_appimage = keg_path.join("AppImages/Test.AppImage");
        fs::create_dir_all(staged_appimage.parent().unwrap()).unwrap();
        fs::write(&staged_appimage, b"appimage").unwrap();
        let appimage_target = appimage_dir.join("Test.AppImage");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&staged_appimage, &appimage_target).unwrap();

        Installer::cleanup_failed_install(
            &linker,
            &cellar,
            "cask:test",
            "1.0.0",
            &keg_path,
            &appimage_dir,
            true,
        );

        assert!(!app_target.exists());
        assert!(!font_target.exists());
        assert!(appimage_target.symlink_metadata().is_err());
        assert!(!keg_path.exists());
    }
}
