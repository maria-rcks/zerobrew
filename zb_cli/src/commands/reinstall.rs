use crate::commands::install::{self, InstallRequest};
use crate::ui::StdUi;
use crate::utils::{PackageKind, normalize_package_name};
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

    for name in names {
        if request.ask {
            ui.println(format!("Would reinstall 1 formula:\n    {name}"))
                .map_err(ui_error)?;
            continue;
        }
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

fn ui_error(err: std::io::Error) -> zb_core::Error {
    zb_core::Error::FileError {
        message: format!("failed to write CLI output: {err}"),
    }
}
