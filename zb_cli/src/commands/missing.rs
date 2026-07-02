use crate::ui::Ui;

pub async fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
    hide: Vec<String>,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    let missing = installer.missing_dependencies(&formulas, &hide).await?;
    let multiple = formulas.len() != 1;

    for (formula, dependencies) in &missing {
        if multiple {
            ui.data(format!("{formula}: {}", dependencies.join(" ")));
        } else {
            ui.data(dependencies.join(" "));
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(zb_core::Error::ExecutionError {
            message: "missing dependencies".to_string(),
        })
    }
}
