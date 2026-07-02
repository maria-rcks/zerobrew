use console::style;

use crate::ui::Ui;

pub fn execute(
    installer: &mut zb_io::Installer,
    repair: bool,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    ui.heading("Running diagnostics...");

    let report = installer.doctor()?;

    if report.is_healthy() {
        ui.success("No issues found");
        return Ok(());
    }

    for orphan in &report.orphaned_cellar_kegs {
        ui.warn(format!(
            "Orphaned cellar keg: {}/{} (no DB record)",
            orphan.name, orphan.version
        ));
    }

    for missing in &report.missing_cellar_kegs {
        ui.warn(format!(
            "Missing cellar keg: {}/{} (DB record exists but {} is gone)",
            missing.name,
            missing.version,
            missing.expected_path.display()
        ));
    }

    for key in &report.orphaned_store_entries {
        ui.warn(format!(
            "Orphaned store entry: {} (no DB reference)",
            &key[..key.len().min(12)]
        ));
    }

    for stale in &report.stale_store_refs {
        let status = if !stale.on_disk {
            "not on disk"
        } else if !stale.referenced_by_any_keg {
            "unreferenced"
        } else {
            "refcount mismatch"
        };
        ui.warn(format!(
            "Stale store ref: {} (refcount={}, {})",
            &stale.store_key[..stale.store_key.len().min(12)],
            stale.refcount,
            status
        ));
    }

    for link in &report.broken_symlinks {
        ui.warn(format!("Broken symlink: {}", link.display()));
    }

    if report.stale_keg_file_records > 0 {
        ui.warn(format!(
            "{} stale keg_files records (referencing uninstalled kegs)",
            report.stale_keg_file_records
        ));
    }

    let issue_count = report.orphaned_cellar_kegs.len()
        + report.missing_cellar_kegs.len()
        + report.orphaned_store_entries.len()
        + report.stale_store_refs.len()
        + report.broken_symlinks.len()
        + usize::from(report.stale_keg_file_records > 0);

    ui.blank_line();
    ui.heading(format!(
        "Found {} {}",
        style(issue_count).yellow().bold(),
        if issue_count == 1 { "issue" } else { "issues" }
    ));

    if !repair {
        ui.status(format!(
            "    Run {} to fix",
            style("zb doctor --repair").bold()
        ));
        return Ok(());
    }

    ui.blank_line();
    ui.heading("Repairing...");

    let summary = installer.repair(&report)?;

    if summary.removed_orphaned_kegs > 0 {
        ui.bullet(format!(
            "Removed {} orphaned cellar {}",
            summary.removed_orphaned_kegs,
            pluralize("keg", summary.removed_orphaned_kegs)
        ));
    }
    if summary.removed_missing_records > 0 {
        ui.bullet(format!(
            "Removed {} stale DB {}",
            summary.removed_missing_records,
            pluralize("record", summary.removed_missing_records)
        ));
    }
    if summary.fixed_store_refs > 0 {
        ui.bullet(format!(
            "Fixed {} store {}",
            summary.fixed_store_refs,
            pluralize("ref", summary.fixed_store_refs)
        ));
    }
    if summary.removed_orphaned_store_entries > 0 {
        ui.bullet(format!(
            "Removed {} orphaned store {}",
            summary.removed_orphaned_store_entries,
            pluralize("entry", summary.removed_orphaned_store_entries)
        ));
    }
    if summary.removed_broken_symlinks > 0 {
        ui.bullet(format!(
            "Removed {} broken {}",
            summary.removed_broken_symlinks,
            pluralize("symlink", summary.removed_broken_symlinks)
        ));
    }
    if summary.pruned_keg_file_records > 0 {
        ui.bullet(format!(
            "Pruned {} stale keg_files {}",
            summary.pruned_keg_file_records,
            pluralize("record", summary.pruned_keg_file_records)
        ));
    }

    ui.blank_line();
    ui.success(format!(
        "Applied {} {}",
        summary.total_fixes(),
        pluralize("fix", summary.total_fixes())
    ));

    Ok(())
}

fn pluralize(word: &str, count: usize) -> &str {
    if count == 1 {
        word
    } else {
        match word {
            "keg" => "kegs",
            "record" => "records",
            "ref" => "refs",
            "entry" => "entries",
            "symlink" => "symlinks",
            "fix" => "fixes",
            "issue" => "issues",
            _ => word,
        }
    }
}
