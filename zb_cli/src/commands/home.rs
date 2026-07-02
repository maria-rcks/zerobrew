use crate::ui::Ui;

pub async fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    if formulas.is_empty() {
        ui.data("https://github.com/maria-rcks/zerobrew");
        return Ok(());
    }

    for formula in formulas {
        let formula = installer.formula_metadata(&formula).await?;
        let homepage = formula
            .homepage
            .as_deref()
            .unwrap_or("https://github.com/maria-rcks/zerobrew");
        ui.data(homepage);
    }
    Ok(())
}
