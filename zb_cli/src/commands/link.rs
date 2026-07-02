use console::style;

use crate::ui::Ui;
use crate::utils::{PackageKind, normalize_package_name};

pub fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    let names = normalize_formula_names(formulas)?;

    for name in names {
        ui.heading(format!("Linking {}...", style(&name).bold()));
        let linked = installer.link_installed(&name)?;
        ui.status(format!(
            "    Linked {} {}.",
            style(linked.len()).green().bold(),
            if linked.len() == 1 {
                "symlink"
            } else {
                "symlinks"
            }
        ));
    }

    Ok(())
}

fn normalize_formula_names(formulas: Vec<String>) -> Result<Vec<String>, zb_core::Error> {
    formulas
        .into_iter()
        .map(|formula| normalize_package_name(&formula, PackageKind::Formula))
        .collect()
}
