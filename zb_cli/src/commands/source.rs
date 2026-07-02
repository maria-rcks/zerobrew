use crate::ui::Ui;

pub async fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    for formula in formulas {
        ui.data_raw(installer.formula_source(&formula).await?);
    }
    Ok(())
}
