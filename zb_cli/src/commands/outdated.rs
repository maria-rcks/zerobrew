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

    print_outdated_packages(&outdated, ui);

    Ok(())
}

fn print_outdated_packages(outdated: &[zb_io::OutdatedPackage], ui: &mut Ui) {
    for pkg in outdated {
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
}

#[cfg(test)]
mod tests {
    use super::print_outdated_packages;
    use crate::ui::{Ui, UiOptions};

    fn package() -> zb_io::OutdatedPackage {
        zb_io::OutdatedPackage {
            name: "jq".to_string(),
            installed_version: "1.6".to_string(),
            current_version: "1.7.1".to_string(),
            installed_sha256: "old".to_string(),
            current_sha256: "new".to_string(),
            is_source_build: false,
        }
    }

    #[test]
    fn quiet_mode_prints_names_only() {
        let (mut ui, out, err) = Ui::for_test(UiOptions {
            quiet: true,
            ..Default::default()
        });

        print_outdated_packages(&[package()], &mut ui);

        assert_eq!(out.contents(), "jq\n");
        assert!(err.contents().is_empty());
    }

    #[test]
    fn default_mode_prints_parenthesized_comparison() {
        let (mut ui, out, err) = Ui::for_test(UiOptions::default());

        print_outdated_packages(&[package()], &mut ui);

        assert_eq!(out.contents(), "jq (1.6) < 1.7.1\n");
        assert!(err.contents().is_empty());
    }

    #[test]
    fn verbose_mode_prints_arrow_comparison() {
        let (mut ui, out, err) = Ui::for_test(UiOptions {
            verbose: 1,
            ..Default::default()
        });

        print_outdated_packages(&[package()], &mut ui);

        assert_eq!(out.contents(), "jq 1.6 → 1.7.1\n");
        assert!(err.contents().is_empty());
    }
}
