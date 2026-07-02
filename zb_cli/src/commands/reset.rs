use std::path::Path;
use std::process::Command;

use zb_io::validate_privileged_path;

use crate::init::{InitError, run_init};
use crate::ui::{PromptDefault, Ui};

pub fn execute(root: &Path, prefix: &Path, yes: bool, ui: &mut Ui) -> Result<(), zb_core::Error> {
    validate_privileged_path(root)?;
    validate_privileged_path(prefix)?;

    if !root.exists() && !prefix.exists() {
        ui.info("Nothing to reset - directories do not exist.");
        return Ok(());
    }

    if !yes {
        ui.note("This will delete all zerobrew data at:");
        ui.bullet(root.display());
        ui.bullet(prefix.display());

        if !ui.confirm("Continue?", PromptDefault::No) {
            ui.info("Aborted.");
            return Ok(());
        }
    }

    for dir in [root, prefix] {
        if !dir.exists() {
            continue;
        }

        ui.heading(format!("Clearing {}...", dir.display()));

        // Instead of removing the directory entirely (which would require sudo to recreate),
        // just remove its contents. This avoids needing sudo when run_init recreates subdirs.
        let mut failed = false;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let result = if path.is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                if result.is_err() {
                    failed = true;
                    break;
                }
            }
        } else {
            failed = true;
        }

        // Only fall back to sudo if we couldn't clear contents AND we can ask the user
        if failed {
            if !ui.is_interactive() {
                return Err(zb_core::Error::FileError {
                    message: format!(
                        "failed to clear {} (permission denied, non-interactive mode)",
                        dir.display()
                    ),
                });
            }

            // Interactive mode: fall back to sudo for the entire directory
            let status = Command::new("sudo")
                .args(["rm", "-rf", &dir.to_string_lossy()])
                .status();

            if !status.is_ok_and(|status| status.success()) {
                return Err(zb_core::Error::FileError {
                    message: format!("failed to remove {}", dir.display()),
                });
            }
        }
    }

    // Pass false for no_modify_shell since this is a re-initialization
    run_init(root, prefix, false, ui).map_err(|e| match e {
        InitError::Message(msg) => zb_core::Error::StoreCorruption { message: msg },
    })?;

    ui.heading("Reset complete. Ready for cold install.");

    Ok(())
}
