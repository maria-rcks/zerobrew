use crate::ui::Ui;

pub async fn execute(
    installer: &mut zb_io::Installer,
    versions: bool,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    if versions {
        for (name, version) in installer.list_formula_versions().await? {
            ui.data(format!("{name} {version}"));
        }
    } else {
        for name in installer.list_formula_names().await? {
            ui.data(name);
        }
    }

    Ok(())
}
