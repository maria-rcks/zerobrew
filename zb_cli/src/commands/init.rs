use std::path::Path;

use crate::init::{InitError, run_init};
use crate::ui::Ui;

pub fn execute(
    root: &Path,
    prefix: &Path,
    no_modify_path: bool,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    run_init(root, prefix, no_modify_path, ui).map_err(|e| match e {
        InitError::Message(msg) => zb_core::Error::StoreCorruption { message: msg },
    })
}
