use crate::ui::{PromptDefault, StdUi};
use console::style;
use std::path::{Path, PathBuf};
use std::process::Command;

pub async fn execute(
    installer: &mut zb_io::Installer,
    yes: bool,
    force: bool,
    ui: &mut StdUi,
) -> Result<(), zb_core::Error> {
    ui.heading("Fetching installed Homebrew packages...")
        .map_err(ui_error)?;

    let packages = zb_io::get_homebrew_packages()?;

    if packages.formulas.is_empty()
        && packages.non_core_formulas.is_empty()
        && packages.casks.is_empty()
    {
        ui.println("No Homebrew packages installed.")
            .map_err(ui_error)?;
        return Ok(());
    }

    ui.println(format!(
        "{} core formulas, {} non-core formulas, {} casks found",
        style(packages.formulas.len()).green(),
        style(packages.non_core_formulas.len()).yellow(),
        style(packages.casks.len()).green()
    ))
    .map_err(ui_error)?;
    ui.blank_line().map_err(ui_error)?;

    let formula_names: Vec<String> = packages
        .formulas
        .iter()
        .chain(&packages.non_core_formulas)
        .map(|f| f.name.clone())
        .collect();
    let (cask_jsons, unsupported_casks) = supported_cask_jsons(&packages.casks);
    let cask_install_names: Vec<String> = cask_jsons
        .iter()
        .map(|(name, _)| format!("cask:{name}"))
        .collect();
    let cask_target_conflicts =
        cask_artifact_conflicts(&cask_jsons, installer.app_dir(), installer.font_dir());
    let cask_fallback_dirs = if cask_target_conflicts.is_empty() {
        None
    } else {
        let app_dir = installer.prefix().join("Applications");
        let font_dir = installer.prefix().join("Fonts");
        Some((app_dir, font_dir))
    };

    if formula_names.is_empty() && cask_install_names.is_empty() {
        ui.println("No supported Homebrew packages to migrate.")
            .map_err(ui_error)?;
        return Ok(());
    }

    if !formula_names.is_empty() {
        ui.println(format!(
            "The following {} formulas will be migrated:",
            formula_names.len()
        ))
        .map_err(ui_error)?;
        for name in &formula_names {
            ui.bullet(name).map_err(ui_error)?;
        }
        ui.blank_line().map_err(ui_error)?;
    }

    if let Some((app_dir, font_dir)) = &cask_fallback_dirs {
        ui.note(format!(
            "Detected {} existing cask app/font target(s). Migrated cask artifacts will be linked under '{}' and '{}' to avoid overwriting existing Homebrew-managed files.",
            cask_target_conflicts.len(),
            app_dir.display(),
            font_dir.display()
        ))
        .map_err(ui_error)?;
        for conflict in cask_target_conflicts.iter().take(5) {
            ui.bullet(conflict.display().to_string())
                .map_err(ui_error)?;
        }
        if cask_target_conflicts.len() > 5 {
            ui.bullet(format!("... and {} more", cask_target_conflicts.len() - 5))
                .map_err(ui_error)?;
        }
        ui.blank_line().map_err(ui_error)?;
    }

    if !unsupported_casks.is_empty() {
        ui.note(format!(
            "Skipping {} unsupported cask(s):",
            unsupported_casks.len()
        ))
        .map_err(ui_error)?;
        for name in &unsupported_casks {
            ui.bullet(name).map_err(ui_error)?;
        }
        ui.blank_line().map_err(ui_error)?;
    }

    if !cask_jsons.is_empty() {
        ui.println(format!(
            "The following {} casks will be migrated:",
            cask_jsons.len()
        ))
        .map_err(ui_error)?;
        for (name, _) in &cask_jsons {
            ui.bullet(name).map_err(ui_error)?;
        }
        ui.blank_line().map_err(ui_error)?;
    }

    if !yes
        && !ui
            .prompt_yes_no("Continue with migration? [y/N]", PromptDefault::No)
            .map_err(ui_error)?
    {
        ui.println("Aborted.").map_err(ui_error)?;
        return Ok(());
    }

    ui.blank_line().map_err(ui_error)?;
    ui.heading(format!(
        "Migrating {} packages to zerobrew...",
        style(formula_names.len() + cask_install_names.len())
            .green()
            .bold()
    ))
    .map_err(ui_error)?;

    if !formula_names.is_empty() {
        crate::commands::install::execute(
            installer,
            crate::commands::install::InstallRequest {
                formulas: formula_names.clone(),
                no_link: false,
                build_from_source: false,
                ignore_dependencies: false,
                only_dependencies: false,
                ask: false,
                cask: false,
                formula: false,
                appdir: None,
                fontdir: None,
                appimagedir: None,
                no_binaries: false,
                force: false,
            },
            ui,
        )
        .await
        .ok();
    }

    if !cask_jsons.is_empty() {
        ui.heading(format!(
            "Installing casks ({} packages)...",
            cask_jsons.len()
        ))
        .map_err(ui_error)?;
        if let Some((app_dir, font_dir)) = cask_fallback_dirs {
            installer.set_cask_artifact_dirs(app_dir, font_dir);
        }
        if let Err(e) = installer.install_casks_from_json(&cask_jsons, true).await {
            ui.error(format!("Cask migration hit an error: {e}"))
                .map_err(ui_error)?;
        }
    }

    let mut expected_names = formula_names.clone();
    expected_names.extend(cask_install_names.clone());
    let (successfully_installed, failed_installed) =
        check_install_status(installer, &expected_names);
    let success_count = successfully_installed.len();

    ui.blank_line().map_err(ui_error)?;
    ui.heading(format!(
        "Migrated {} of {} packages to zerobrew",
        style(success_count).green().bold(),
        expected_names.len()
    ))
    .map_err(ui_error)?;

    if !failed_installed.is_empty() {
        ui.note(format!(
            "Failed to migrate {} package(s):",
            failed_installed.len()
        ))
        .map_err(ui_error)?;
        for name in &failed_installed {
            ui.bullet(name).map_err(ui_error)?;
        }
        ui.blank_line().map_err(ui_error)?;
    }

    if success_count == 0 {
        ui.println("No packages were successfully migrated. Skipping uninstall from Homebrew.")
            .map_err(ui_error)?;
        return Ok(());
    }

    ui.blank_line().map_err(ui_error)?;
    if !yes
        && !ui
            .prompt_yes_no(
                &format!(
                    "Uninstall {} package(s) from Homebrew? [y/N]",
                    style(success_count).green()
                ),
                PromptDefault::No,
            )
            .map_err(ui_error)?
    {
        ui.println("Skipped uninstall from Homebrew.")
            .map_err(ui_error)?;
        return Ok(());
    }

    ui.blank_line().map_err(ui_error)?;
    ui.heading("Uninstalling from Homebrew...")
        .map_err(ui_error)?;

    let uninstall_failed = uninstall_homebrew_packages(&successfully_installed, force, ui)?;

    let uninstalled = successfully_installed.len() - uninstall_failed.len();
    ui.blank_line().map_err(ui_error)?;
    ui.heading(format!(
        "Uninstalled {} of {} package(s) from Homebrew",
        style(uninstalled).green().bold(),
        success_count
    ))
    .map_err(ui_error)?;

    if !uninstall_failed.is_empty() {
        ui.note(format!(
            "Failed to uninstall {} package(s) from Homebrew:",
            uninstall_failed.len()
        ))
        .map_err(ui_error)?;
        for name in &uninstall_failed {
            ui.bullet(name).map_err(ui_error)?;
        }
        ui.println("You may need to uninstall these manually with:")
            .map_err(ui_error)?;
        ui.println("    brew uninstall --force <formula>")
            .map_err(ui_error)?;
        ui.println("    brew uninstall --cask --force <cask>")
            .map_err(ui_error)?;
    }

    Ok(())
}

fn cask_artifact_conflicts(
    casks: &[(String, serde_json::Value)],
    app_dir: &Path,
    font_dir: &Path,
) -> Vec<PathBuf> {
    let mut conflicts = Vec::new();

    for (token, cask_json) in casks {
        let Ok(cask) = zb_io::resolve_cask(token, cask_json) else {
            continue;
        };
        for app in &cask.apps {
            let target = app_dir.join(&app.target);
            if target.symlink_metadata().is_ok() {
                conflicts.push(target);
            }
        }
        for font in &cask.fonts {
            let target = font_dir.join(&font.target);
            if target.symlink_metadata().is_ok() {
                conflicts.push(target);
            }
        }
    }

    conflicts
}

fn supported_cask_jsons(
    casks: &[zb_io::HomebrewPackage],
) -> (Vec<(String, serde_json::Value)>, Vec<String>) {
    let mut supported = Vec::new();
    let mut unsupported = Vec::new();

    for cask in casks {
        let Some(cask_json) = cask.cask_json.clone() else {
            unsupported.push(cask.name.clone());
            continue;
        };

        match zb_io::resolve_cask(&cask.name, &cask_json) {
            Ok(_) => supported.push((cask.name.clone(), cask_json)),
            Err(_) => unsupported.push(cask.name.clone()),
        }
    }

    (supported, unsupported)
}

fn uninstall_homebrew_packages(
    installed: &[String],
    force: bool,
    ui: &mut StdUi,
) -> Result<Vec<String>, zb_core::Error> {
    let (casks, formulas): (Vec<_>, Vec<_>) = installed
        .iter()
        .cloned()
        .partition(|name| name.starts_with("cask:"));
    let mut failed = Vec::new();

    if !formulas.is_empty() {
        ui.step_start(format!("uninstalling {} formulas combined", formulas.len()))
            .map_err(ui_error)?;
        let mut args = vec!["uninstall"];
        if force {
            args.push("--force");
        }
        for target in &formulas {
            args.push(target);
        }

        match Command::new("brew").args(&args).status() {
            Ok(status) if status.success() => ui.step_ok().map_err(ui_error)?,
            Ok(_) | Err(_) => {
                ui.step_fail().map_err(ui_error)?;
                failed.extend(still_installed_formulas(&formulas));
            }
        }
    }

    if !casks.is_empty() {
        ui.step_start(format!("uninstalling {} casks combined", casks.len()))
            .map_err(ui_error)?;
        let cask_tokens: Vec<String> = casks
            .iter()
            .map(|name| name.trim_start_matches("cask:").to_string())
            .collect();
        let mut args = vec!["uninstall", "--cask"];
        if force {
            args.push("--force");
        }
        for target in &cask_tokens {
            args.push(target);
        }

        match Command::new("brew").args(&args).status() {
            Ok(status) if status.success() => ui.step_ok().map_err(ui_error)?,
            Ok(_) | Err(_) => {
                ui.step_fail().map_err(ui_error)?;
                failed.extend(
                    still_installed_casks(&cask_tokens)
                        .into_iter()
                        .map(|name| format!("cask:{name}")),
                );
            }
        }
    }

    Ok(failed)
}

fn still_installed_formulas(targets: &[String]) -> Vec<String> {
    let mut actually_failed = targets.to_vec();
    if let Ok(output) = Command::new("brew")
        .args(["list", "--formula", "--full-name"])
        .output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let still_installed = installed_formula_tokens(&stdout);
        actually_failed.retain(|target| still_installed.contains(target.as_str()));
    }
    actually_failed
}

fn installed_formula_tokens(output: &str) -> std::collections::HashSet<&str> {
    output
        .lines()
        .map(|line| line.rsplit('/').next().unwrap_or(line))
        .collect()
}

fn still_installed_casks(targets: &[String]) -> Vec<String> {
    let mut actually_failed = targets.to_vec();
    if let Ok(output) = Command::new("brew").args(["list", "--cask"]).output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let still_installed: std::collections::HashSet<&str> = stdout.lines().collect();
        actually_failed.retain(|target| still_installed.contains(target.as_str()));
    }
    actually_failed
}

// FIXME: Abstract this return type to a more structured type (e.g., a struct)
fn check_install_status(
    installer: &zb_io::Installer,
    formula_names: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut successfully_installed = Vec::new();
    let mut failed_installed = Vec::new();

    for name in formula_names {
        if installer.is_installed(name) {
            successfully_installed.push(name.clone());
        } else {
            failed_installed.push(name.clone());
        }
    }

    (successfully_installed, failed_installed)
}

fn ui_error(err: std::io::Error) -> zb_core::Error {
    zb_core::Error::StoreCorruption {
        message: format!("failed to write CLI output: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::TempDir;

    use super::{cask_artifact_conflicts, installed_formula_tokens, supported_cask_jsons};

    #[test]
    fn cask_artifact_conflicts_detects_existing_apps_and_fonts() {
        let tmp = TempDir::new().unwrap();
        let app_dir = tmp.path().join("Applications");
        let font_dir = tmp.path().join("Fonts");
        std::fs::create_dir_all(app_dir.join("Demo.app")).unwrap();
        std::fs::create_dir_all(&font_dir).unwrap();
        std::fs::write(font_dir.join("Demo.otf"), b"font").unwrap();

        let casks = vec![(
            "demo".to_string(),
            json!({
                "version": "1.0.0",
                "url": "https://example.com/demo.zip",
                "sha256": "a".repeat(64),
                "artifacts": [
                    { "app": ["Demo.app"] },
                    { "font": ["Demo.otf"] }
                ]
            }),
        )];

        let conflicts = cask_artifact_conflicts(&casks, &app_dir, &font_dir);

        assert_eq!(conflicts.len(), 2);
        assert!(conflicts.contains(&app_dir.join("Demo.app")));
        assert!(conflicts.contains(&font_dir.join("Demo.otf")));
    }

    #[test]
    fn cask_artifact_conflicts_ignores_missing_targets() {
        let tmp = TempDir::new().unwrap();
        let casks = vec![(
            "demo".to_string(),
            json!({
                "version": "1.0.0",
                "url": "https://example.com/demo.zip",
                "sha256": "a".repeat(64),
                "artifacts": [{ "app": ["Demo.app"] }]
            }),
        )];

        let conflicts =
            cask_artifact_conflicts(&casks, &tmp.path().join("Applications"), tmp.path());

        assert!(conflicts.is_empty());
    }

    #[test]
    fn cask_artifact_conflicts_skips_unsupported_casks() {
        let tmp = TempDir::new().unwrap();
        let casks = vec![(
            "pkg-only".to_string(),
            json!({
                "version": "1.0.0",
                "url": "https://example.com/pkg-only.pkg",
                "sha256": "a".repeat(64),
                "artifacts": [{ "pkg": ["Pkg.pkg"] }]
            }),
        )];

        let conflicts =
            cask_artifact_conflicts(&casks, &tmp.path().join("Applications"), tmp.path());

        assert!(conflicts.is_empty());
    }

    #[test]
    fn supported_cask_jsons_skips_unsupported_casks() {
        let packages = vec![
            zb_io::HomebrewPackage {
                name: "demo".to_string(),
                tap: "homebrew/cask".to_string(),
                is_cask: true,
                cask_json: Some(json!({
                    "version": "1.0.0",
                    "url": "https://example.com/demo.zip",
                    "sha256": "a".repeat(64),
                    "artifacts": [{ "app": ["Demo.app"] }]
                })),
            },
            zb_io::HomebrewPackage {
                name: "zap-only".to_string(),
                tap: "homebrew/cask".to_string(),
                is_cask: true,
                cask_json: Some(json!({
                    "version": "1.0.0",
                    "url": "https://example.com/zap-only.zip",
                    "sha256": "b".repeat(64),
                    "artifacts": [{ "zap": [{ "trash": ["~/Library/Application Support/Demo"] }] }]
                })),
            },
        ];

        let (supported, unsupported) = supported_cask_jsons(&packages);

        assert_eq!(supported.len(), 1);
        assert_eq!(supported[0].0, "demo");
        assert_eq!(unsupported, vec!["zap-only"]);
    }

    #[test]
    fn installed_formula_tokens_normalizes_tap_prefixed_names() {
        let installed = installed_formula_tokens("bash\nuser/tap/cppzmq\n");

        assert!(installed.contains("bash"));
        assert!(installed.contains("cppzmq"));
        assert!(!installed.contains("user/tap/cppzmq"));
    }
}
