use console::style;

use crate::ui::Ui;

pub fn execute(installer: &mut zb_io::Installer, ui: &mut Ui) -> Result<(), zb_core::Error> {
    ui.heading("Running garbage collection...");
    let removed = installer.gc()?;

    if removed.is_empty() {
        ui.status("No unreferenced store entries to remove.");
    } else {
        for key in &removed {
            ui.success(format!("Removed {}", &key[..12]));
        }
        ui.heading(format!(
            "Removed {} store entries",
            style(removed.len()).green().bold()
        ));
    }

    Ok(())
}
