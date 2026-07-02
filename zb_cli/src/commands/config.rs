use std::path::Path;

use crate::ui::Ui;

pub fn execute(root: &Path, prefix: &Path, ui: &mut Ui) -> Result<(), zb_core::Error> {
    ui.data(format!("ZEROBREW_ROOT: {}", root.display()));
    ui.data(format!("HOMEBREW_PREFIX: {}", prefix.display()));
    ui.data(format!(
        "HOMEBREW_CELLAR: {}",
        prefix.join("Cellar").display()
    ));
    ui.data(format!("ZEROBREW_CACHE: {}", root.join("cache").display()));
    ui.data(format!("ZEROBREW_STORE: {}", root.join("store").display()));
    Ok(())
}
