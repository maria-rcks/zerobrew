use crate::ui::Ui;

pub async fn execute(installer: &mut zb_io::Installer, ui: &mut Ui) -> Result<(), zb_core::Error> {
    for keg in installer.list_leaves().await? {
        ui.data(keg.name);
    }

    Ok(())
}
