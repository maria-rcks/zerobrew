use chrono::{DateTime, Local};
use console::style;

pub fn execute(installer: &mut zb_io::Installer, formula: String) -> Result<(), zb_core::Error> {
    if let Some(keg) = installer.get_installed(&formula) {
        print_field("Name:", style(&keg.name).bold());
        print_field("Version:", &keg.version);
        print_field("Store key:", store_key_prefix(&keg.store_key));
        print_field("Installed:", format_timestamp(keg.installed_at));
    } else {
        println!("Formula '{}' is not installed.", formula);
    }

    Ok(())
}

fn store_key_prefix(store_key: &str) -> &str {
    &store_key[..store_key.len().min(12)]
}

fn print_field(label: &str, value: impl std::fmt::Display) {
    println!("{:<10}  {}", style(label).dim(), value);
}

fn format_timestamp(timestamp: i64) -> String {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => {
            let local_dt = dt.with_timezone(&Local);
            let now = Local::now();
            let duration = now.signed_duration_since(local_dt);

            if duration.num_days() > 0 {
                format!(
                    "{} ({} days ago)",
                    local_dt.format("%Y-%m-%d"),
                    duration.num_days()
                )
            } else if duration.num_hours() > 0 {
                format!(
                    "{} ({} hours ago)",
                    local_dt.format("%Y-%m-%d %H:%M"),
                    duration.num_hours()
                )
            } else {
                format!(
                    "{} ({} minutes ago)",
                    local_dt.format("%H:%M"),
                    duration.num_minutes()
                )
            }
        }
        None => "invalid timestamp".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::store_key_prefix;

    #[test]
    fn store_key_prefix_handles_short_keys() {
        assert_eq!(store_key_prefix("cellar-only"), "cellar-only");
    }

    #[test]
    fn store_key_prefix_truncates_long_keys() {
        assert_eq!(store_key_prefix("1234567890abcdef"), "1234567890ab");
    }
}
