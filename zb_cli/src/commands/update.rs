use console::style;

use crate::ui::Ui;

pub fn execute(installer: &mut zb_io::Installer, ui: &mut Ui) -> Result<(), zb_core::Error> {
    let removed = installer.clear_api_cache()?;
    if removed == 0 {
        ui.heading("No cached entries to clear.");
    } else {
        ui.heading(format!(
            "Cleared {} cached formula {}.",
            style(removed).green().bold(),
            if removed == 1 { "entry" } else { "entries" }
        ));
    }
    ui.status(style("Run `zb outdated` to check for updates.").dim());
    Ok(())
}
