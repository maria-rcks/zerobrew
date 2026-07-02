use crate::ui::Ui;
use crate::utils::{PackageKind, normalize_package_name};
use console::style;

pub fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
    all: bool,
    cask: bool,
    formula: bool,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    let kind = if cask {
        PackageKind::Cask
    } else if formula {
        PackageKind::Formula
    } else {
        PackageKind::Auto
    };

    let formulas = if all {
        let installed: Vec<String> = installer
            .list_installed()?
            .into_iter()
            .map(|k| k.name)
            .filter(|name| match kind {
                PackageKind::Cask => name.starts_with("cask:"),
                PackageKind::Formula => !name.starts_with("cask:"),
                PackageKind::Auto => true,
            })
            .collect();
        if installed.is_empty() {
            ui.info("No matching packages installed.");
            return Ok(());
        }
        installed
    } else {
        let mut normalized = Vec::with_capacity(formulas.len());
        for formula in formulas {
            normalized.push(normalize_package_name(&formula, kind)?);
        }
        normalized
    };

    ui.heading(format!(
        "Uninstalling {}...",
        style(formulas.join(", ")).bold()
    ));

    let mut errors: Vec<(String, zb_core::Error)> = Vec::new();

    if formulas.len() > 1 {
        for name in &formulas {
            ui.step_start(name);
            match installer.uninstall(name) {
                Ok(()) => ui.step_ok(),
                Err(e) => {
                    ui.step_fail();
                    errors.push((name.clone(), e));
                }
            }
        }
    } else if let Err(e) = installer.uninstall(&formulas[0]) {
        errors.push((formulas[0].clone(), e));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        for (name, err) in &errors {
            ui.error(format!(
                "Failed to uninstall {}: {}",
                style(name).bold(),
                err
            ));
        }
        // Return just the first error up. TODO: don't return errors from this fn?
        Err(errors.remove(0).1)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;
    use zb_core::formula_token;
    use zb_io::{ApiClient, BlobCache, Cellar, Database, Installer, Linker, Store};

    use super::execute;
    use crate::ui::{Ui, UiOptions};

    fn installer_with_installed(
        root: &std::path::Path,
        prefix: &std::path::Path,
        installed: &[&str],
    ) -> Installer {
        fs::create_dir_all(root.join("db")).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(root).unwrap();
        let cellar = Cellar::new(root).unwrap();
        let linker = Linker::new(prefix).unwrap();
        let mut db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        {
            let tx = db.transaction().unwrap();
            for name in installed {
                tx.record_install(name, "1.0.0", &format!("{name}-store"))
                    .unwrap();
                fs::create_dir_all(cellar.keg_path(formula_token(name), "1.0.0")).unwrap();
            }
            tx.commit().unwrap();
        }

        Installer::new(
            ApiClient::new(),
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.to_path_buf(),
            root.join("locks"),
        )
    }

    #[test]
    fn all_formula_uninstalls_only_formulas() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer =
            installer_with_installed(&root, &prefix, &["cask:raycast", "jq", "zlib"]);
        let (mut ui, out, _err) = Ui::for_test(UiOptions::default());

        execute(&mut installer, Vec::new(), true, false, true, &mut ui).unwrap();

        let remaining = installer
            .list_installed()
            .unwrap()
            .into_iter()
            .map(|keg| keg.name)
            .collect::<Vec<_>>();
        assert_eq!(remaining, vec!["cask:raycast"]);
        assert!(out.contents().is_empty());
    }

    #[test]
    fn all_cask_uninstalls_only_casks() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer =
            installer_with_installed(&root, &prefix, &["cask:raycast", "jq", "zlib"]);
        let (mut ui, out, _err) = Ui::for_test(UiOptions::default());

        execute(&mut installer, Vec::new(), true, true, false, &mut ui).unwrap();

        let remaining = installer
            .list_installed()
            .unwrap()
            .into_iter()
            .map(|keg| keg.name)
            .collect::<Vec<_>>();
        assert_eq!(remaining, vec!["jq", "zlib"]);
        assert!(out.contents().is_empty());
    }
}
