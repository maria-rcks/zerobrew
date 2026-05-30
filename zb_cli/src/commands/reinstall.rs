use crate::commands::install::{self, InstallRequest};
use crate::ui::{PromptDefault, StdUi, Ui};
use crate::utils::{PackageKind, normalize_package_name};
use std::io::{BufRead, Write};
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
    ui: &mut StdUi,
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
        let mut stdin = std::io::stdin().lock();
        if !confirm_reinstall_with_reader(ui, &names, &mut stdin)? {
            return Ok(());
        }
    }

    for name in names {
        ui.println(format!("Reinstalling {name}"))
            .map_err(ui_error)?;
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

fn confirm_reinstall_with_reader<O: Write, E: Write, R: BufRead>(
    ui: &mut Ui<O, E>,
    names: &[String],
    reader: &mut R,
) -> Result<bool, zb_core::Error> {
    ui.println(format_reinstall_plan(names)).map_err(ui_error)?;
    let answer = ui
        .prompt_yes_no_with_reader("Reinstall these formulae? [y/N]", PromptDefault::No, reader)
        .map_err(ui_error)?;
    if !answer {
        ui.println("Reinstall cancelled.").map_err(ui_error)?;
    }
    Ok(answer)
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

fn ui_error(err: std::io::Error) -> zb_core::Error {
    zb_core::Error::FileError {
        message: format!("failed to write CLI output: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::confirm_reinstall_with_reader;
    use crate::ui::Ui;
    use std::io::Cursor;

    #[test]
    fn confirm_reinstall_defaults_to_cancel() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ui = Ui::with_writers(&mut stdout, &mut stderr);
        let mut input = Cursor::new("\n");
        let names = vec!["jq".to_string()];

        let accepted = confirm_reinstall_with_reader(&mut ui, &names, &mut input).unwrap();
        drop(ui);

        assert!(!accepted);
        let output = String::from_utf8(stdout).unwrap();
        assert!(output.contains("Would reinstall 1 formula:\n    jq"));
        assert!(output.contains("Reinstall these formulae? [y/N]"));
        assert!(output.contains("Reinstall cancelled."));
    }

    #[test]
    fn confirm_reinstall_accepts_yes() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ui = Ui::with_writers(&mut stdout, &mut stderr);
        let mut input = Cursor::new("y\n");
        let names = vec!["jq".to_string(), "ripgrep".to_string()];

        let accepted = confirm_reinstall_with_reader(&mut ui, &names, &mut input).unwrap();
        drop(ui);

        assert!(accepted);
        let output = String::from_utf8(stdout).unwrap();
        assert!(output.contains("Would reinstall 2 formulae:\n    jq\n    ripgrep"));
        assert!(!output.contains("Reinstall cancelled."));
    }
}
