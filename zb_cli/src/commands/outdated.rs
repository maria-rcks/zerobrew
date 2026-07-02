use console::style;

use crate::ui::Ui;

pub async fn execute(
    installer: &mut zb_io::Installer,
    json: bool,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    let (outdated, warnings) = installer.check_outdated().await?;

    // Warnings always go to stderr (never pollute stdout, especially in --json mode)
    for warning in &warnings {
        ui.warn(warning);
    }

    if json {
        let json_output: Vec<serde_json::Value> = outdated
            .iter()
            .map(|pkg| {
                serde_json::json!({
                    "name": pkg.name,
                    "installed_versions": [pkg.installed_version],
                    "current_version": pkg.current_version,
                })
            })
            .collect();
        ui.data_json(&json_output)
            .map_err(|e| zb_core::Error::ExecutionError {
                message: format!("failed to serialize JSON output: {e}"),
            })?;
        return Ok(());
    }

    if outdated.is_empty() {
        ui.heading("All packages are up to date.");
        return Ok(());
    }

    for pkg in &outdated {
        if ui.is_quiet() {
            ui.data(&pkg.name);
        } else if ui.verbose() > 0 {
            ui.data(format!(
                "{} {} {} {}",
                pkg.name,
                style(&pkg.installed_version).red(),
                style("→").dim(),
                style(&pkg.current_version).green(),
            ));
        } else {
            ui.data(format!(
                "{} ({}) < {}",
                pkg.name, pkg.installed_version, pkg.current_version
            ));
        }
    }

    Ok(())
}
