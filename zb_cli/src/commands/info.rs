use chrono::{DateTime, Local};
use console::style;

use crate::ui::Ui;

pub async fn execute(
    installer: &mut zb_io::Installer,
    formula: String,
    show_versions: bool,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    if show_versions {
        print_versions(installer, &formula, ui).await?;
        return Ok(());
    }

    if let Some(keg) = installer.get_installed(&formula) {
        print_field(ui, "Name:", style(&keg.name).bold());
        print_field(ui, "Version:", &keg.version);
        print_field(ui, "Store key:", store_key_prefix(&keg.store_key));
        print_field(ui, "Installed:", format_timestamp(keg.installed_at));
    } else {
        ui.status(format!("Formula '{}' is not installed.", formula));
    }

    Ok(())
}

async fn print_versions(
    installer: &mut zb_io::Installer,
    formula: &str,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    let formula_info = installer.formula_metadata(formula).await?;
    ui.data(format!(
        "{} {}",
        formula_info.name,
        formula_info.effective_version()
    ));
    Ok(())
}

fn store_key_prefix(store_key: &str) -> &str {
    &store_key[..store_key.len().min(12)]
}

fn print_field(ui: &mut Ui, label: &str, value: impl std::fmt::Display) {
    // Width is applied to the StyledObject directly (not a pre-rendered
    // String) so padding stays correct when color is enabled.
    ui.data(format!("{:<10}  {}", style(label).dim(), value));
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
    use std::fs;

    use super::{print_field, print_versions, store_key_prefix};
    use crate::ui::{Ui, UiOptions};
    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use zb_io::{ApiClient, BlobCache, Cellar, Database, Installer, Linker, Store};

    fn test_installer(
        root: &std::path::Path,
        prefix: &std::path::Path,
        api_url: String,
    ) -> Installer {
        fs::create_dir_all(root.join("db")).unwrap();
        let api_client = ApiClient::with_base_url(api_url).expect("test API URL should be valid");
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(root).unwrap();
        let cellar = Cellar::new(root).unwrap();
        let linker = Linker::new(prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();
        Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.to_path_buf(),
            root.join("locks"),
        )
    }

    #[test]
    fn store_key_prefix_handles_short_keys() {
        assert_eq!(store_key_prefix("cellar-only"), "cellar-only");
    }

    #[test]
    fn store_key_prefix_truncates_long_keys() {
        assert_eq!(store_key_prefix("1234567890abcdef"), "1234567890ab");
    }

    #[test]
    fn print_field_writes_aligned_data_to_stdout() {
        let (mut ui, out, err) = Ui::for_test(UiOptions::default());

        print_field(&mut ui, "Name:", "jq");

        assert_eq!(out.contents(), "Name:       jq\n");
        assert!(err.contents().is_empty());
    }

    #[tokio::test]
    async fn print_versions_fetches_formula_metadata() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer =
            test_installer(&root, &prefix, format!("{}/formula", mock_server.uri()));

        Mock::given(method("GET"))
            .and(path("/formula/jq.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"name":"jq","versions":{"stable":"1.7.1"},"revision":2,"dependencies":[],"bottle":{"stable":{"files":{}}}}"#,
            ))
            .mount(&mock_server)
            .await;

        let (mut ui, out, err) = Ui::for_test(UiOptions::default());
        print_versions(&mut installer, "jq", &mut ui).await.unwrap();

        let requests = mock_server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url.path(), "/formula/jq.json");

        // Version output is data: stdout only.
        assert_eq!(out.contents(), "jq 1.7.1_2\n");
        assert!(err.contents().is_empty());
    }
}
