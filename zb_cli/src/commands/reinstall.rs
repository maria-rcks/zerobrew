use crate::commands::install::{self, InstallRequest};
use crate::ui::{PromptDefault, Ui};
use crate::utils::{PackageKind, normalize_package_name};
#[cfg(test)]
use std::io::BufRead;
use std::path::PathBuf;

pub struct ReinstallRequest {
    pub formulas: Vec<String>,
    pub no_link: bool,
    pub build_from_source: bool,
    pub ask: bool,
    pub cask: bool,
    pub formula: bool,
    pub appdir: Option<PathBuf>,
    pub fontdir: Option<PathBuf>,
    pub appimagedir: Option<PathBuf>,
    pub no_binaries: bool,
    pub force: bool,
}

pub async fn execute(
    installer: &mut zb_io::Installer,
    request: ReinstallRequest,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    let kind = if request.cask {
        PackageKind::Cask
    } else if request.formula {
        PackageKind::Formula
    } else {
        PackageKind::Auto
    };
    let names = request
        .formulas
        .iter()
        .map(|formula| normalize_package_name(formula, kind))
        .collect::<Result<Vec<_>, _>>()?;

    for name in &names {
        if !installer.is_installed(name) {
            return Err(zb_core::Error::NotInstalled { name: name.clone() });
        }
    }

    if request.ask {
        ui.status(format_reinstall_plan(&names));
        // `Ui::confirm` reads piped stdin too and treats EOF as the default
        // (No), so this never hangs in non-interactive sessions.
        if !ui.confirm("Reinstall these formulae?", PromptDefault::No) {
            ui.status("Reinstall cancelled.");
            return Ok(());
        }
    }

    for name in names {
        ui.status(format!("Reinstalling {name}"));
        installer.uninstall(&name)?;
        install::execute(
            installer,
            InstallRequest {
                formulas: vec![name],
                no_link: request.no_link,
                build_from_source: request.build_from_source,
                ignore_dependencies: false,
                only_dependencies: false,
                ask: false,
                cask: request.cask,
                formula: request.formula,
                appdir: request.appdir.clone(),
                fontdir: request.fontdir.clone(),
                appimagedir: request.appimagedir.clone(),
                no_binaries: request.no_binaries,
                force: request.force,
            },
            ui,
        )
        .await?;
    }

    Ok(())
}

/// Test mirror of the `--ask` flow in `execute` (plan, prompt, cancel note).
#[cfg(test)]
fn confirm_reinstall_with_reader<R: BufRead>(
    ui: &mut Ui,
    names: &[String],
    reader: &mut R,
) -> bool {
    ui.status(format_reinstall_plan(names));
    let answer = ui.confirm_with_reader("Reinstall these formulae?", PromptDefault::No, reader);
    if !answer {
        ui.status("Reinstall cancelled.");
    }
    answer
}

fn format_reinstall_plan(names: &[String]) -> String {
    let package_label = if names.len() == 1 {
        "formula"
    } else {
        "formulae"
    };
    let mut output = format!("Would reinstall {} {package_label}:", names.len());
    for name in names {
        output.push_str("\n    ");
        output.push_str(name);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::confirm_reinstall_with_reader;
    use crate::ui::{Ui, UiOptions};
    use std::io::Cursor;

    #[test]
    fn confirm_reinstall_defaults_to_cancel() {
        let (mut ui, _out, err) = Ui::for_test(UiOptions::default());
        let mut input = Cursor::new("\n");
        let names = vec!["jq".to_string()];

        let accepted = confirm_reinstall_with_reader(&mut ui, &names, &mut input);

        assert!(!accepted);
        let output = err.contents();
        assert!(output.contains("Would reinstall 1 formula:\n    jq"));
        assert!(output.contains("Reinstall these formulae? [y/N]"));
        assert!(output.contains("Reinstall cancelled."));
    }

    #[test]
    fn confirm_reinstall_accepts_yes() {
        let (mut ui, _out, err) = Ui::for_test(UiOptions::default());
        let mut input = Cursor::new("y\n");
        let names = vec!["jq".to_string(), "ripgrep".to_string()];

        let accepted = confirm_reinstall_with_reader(&mut ui, &names, &mut input);

        assert!(accepted);
        let output = err.contents();
        assert!(output.contains("Would reinstall 2 formulae:\n    jq\n    ripgrep"));
        assert!(!output.contains("Reinstall cancelled."));
    }
}
