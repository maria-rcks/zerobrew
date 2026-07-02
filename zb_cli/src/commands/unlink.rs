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
        ui.heading(format!("Unlinking {}...", style(&name).bold()));
        let unlinked = installer.unlink_installed(&name)?;
        ui.status(format!(
            "    Unlinked {} {}.",
            style(unlinked.len()).green().bold(),
            if unlinked.len() == 1 {
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
