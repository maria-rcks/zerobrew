use console::style;

use crate::commands::install::{self, InstallRequest};
use crate::ui::StdUi;
use crate::utils::{PackageKind, normalize_package_name};
use std::path::PathBuf;

pub struct UpgradeRequest {
    pub formulas: Vec<String>,
    pub dry_run: bool,
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
    request: UpgradeRequest,
    ui: &mut StdUi,
) -> Result<(), zb_core::Error> {
    let kind = if request.cask {
        PackageKind::Cask
    } else if request.formula {
        PackageKind::Formula
    } else {
        PackageKind::Auto
    };

    let names = if request.formulas.is_empty() {
        let (outdated, warnings) = installer.check_outdated().await?;
        for warning in warnings {
            ui.warn(warning).map_err(ui_error)?;
        }
        outdated
            .into_iter()
            .filter(|pkg| package_matches_kind(&pkg.name, kind))
            .map(|pkg| pkg.name)
            .collect()
    } else {
        request
            .formulas
            .iter()
            .map(|formula| normalize_package_name(formula, kind))
            .collect::<Result<Vec<_>, _>>()?
    };

    if names.is_empty() {
        ui.println(format!(
            "{} All packages are up to date.",
            style("==>").cyan().bold()
        ))
        .map_err(ui_error)?;
        return Ok(());
    }

    if request.dry_run {
        for name in names {
            println!("{name}");
        }
        return Ok(());
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

fn package_matches_kind(name: &str, kind: PackageKind) -> bool {
    match kind {
        PackageKind::Auto => true,
        PackageKind::Formula => !name.starts_with("cask:"),
        PackageKind::Cask => name.starts_with("cask:"),
    }
}

fn ui_error(err: std::io::Error) -> zb_core::Error {
    zb_core::Error::StoreCorruption {
        message: format!("failed to write CLI output: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::PackageKind;

    use super::package_matches_kind;

    #[test]
    fn package_kind_filter_matches_formula_and_cask_names() {
        assert!(package_matches_kind("wget", PackageKind::Auto));
        assert!(package_matches_kind("cask:firefox", PackageKind::Auto));
        assert!(package_matches_kind("wget", PackageKind::Formula));
        assert!(!package_matches_kind("cask:firefox", PackageKind::Formula));
        assert!(package_matches_kind("cask:firefox", PackageKind::Cask));
        assert!(!package_matches_kind("wget", PackageKind::Cask));
    }
}
