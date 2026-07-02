use console::style;

use crate::ui::Ui;

pub fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
    versions: bool,
    json: bool,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    let installed = installer.list_installed()?;
    if !formulas.is_empty() {
        for formula in &formulas {
            if !installed.iter().any(|keg| &keg.name == formula) {
                return Err(zb_core::Error::NotInstalled {
                    name: formula.clone(),
                });
            }
        }
    }

    let installed: Vec<_> = if formulas.is_empty() {
        installed
    } else {
        installed
            .into_iter()
            .filter(|keg| formulas.iter().any(|formula| formula == &keg.name))
            .collect()
    };

    if json {
        let packages: Vec<serde_json::Value> = installed
            .iter()
            .map(|keg| {
                if versions {
                    serde_json::json!({
                        "name": keg.name,
                        "versions": [keg.version],
                    })
                } else {
                    serde_json::json!(keg.name)
                }
            })
            .collect();
        ui.data_json(&packages)
            .map_err(|e| zb_core::Error::ExecutionError {
                message: format!("failed to serialize JSON output: {e}"),
            })?;
        return Ok(());
    }

    if installed.is_empty() {
        ui.status("No formulas installed.");
    } else {
        for keg in installed {
            if versions {
                ui.data(format!("{} {}", keg.name, keg.version));
            } else {
                ui.data(style(&keg.name).bold());
            }
        }
    }

    Ok(())
}
