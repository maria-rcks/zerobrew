use console::style;

use crate::ui::Ui;

pub async fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
    include_build: bool,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    let multiple = formulas.len() > 1;
    for formula in formulas {
        let dependencies = installer
            .formula_dependencies(&formula, include_build)
            .await?;
        if multiple {
            ui.data(style(&formula).bold());
        }
        for dependency in dependencies {
            ui.data(dependency);
        }
    }

    Ok(())
}
