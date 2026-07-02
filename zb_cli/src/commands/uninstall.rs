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
    let formulas = if all {
        let installed = installer.list_installed()?;
        if installed.is_empty() {
            ui.info("No formulas installed.");
            return Ok(());
        }
        installed.into_iter().map(|k| k.name).collect()
    } else {
        let mut normalized = Vec::with_capacity(formulas.len());
        let kind = if cask {
            PackageKind::Cask
        } else if formula {
            PackageKind::Formula
        } else {
            PackageKind::Auto
        };
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
