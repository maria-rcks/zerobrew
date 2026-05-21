use crate::commands::install::{self, InstallRequest};
use crate::ui::StdUi;
use crate::utils::{PackageKind, normalize_package_name};
use std::path::PathBuf;

pub struct ReinstallRequest {
    pub formulas: Vec<String>,
    pub no_link: bool,
    pub build_from_source: bool,
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

    for name in &names {
        installer.uninstall(name)?;
    }

    install::execute(
        installer,
        InstallRequest {
            formulas: names,
            no_link: request.no_link,
            build_from_source: request.build_from_source,
            cask: request.cask,
            formula: request.formula,
            appdir: request.appdir,
            fontdir: request.fontdir,
            appimagedir: request.appimagedir,
            no_binaries: request.no_binaries,
            force: request.force,
        },
        ui,
    )
    .await
}
