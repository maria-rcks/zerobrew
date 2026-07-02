use crate::ui::Ui;

pub async fn execute(installer: &mut zb_io::Installer, ui: &mut Ui) -> Result<(), zb_core::Error> {
    for token in installer.list_cask_tokens().await? {
        ui.data(token);
    }

    Ok(())
}
