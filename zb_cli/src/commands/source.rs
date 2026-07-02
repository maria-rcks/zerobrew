use crate::ui::Ui;

pub async fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    let mut first = true;
    for formula in formulas {
        if !first {
            ui.data_raw("\n");
        }
        ui.data_raw(installer.formula_source(&formula).await?);
        first = false;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use zb_io::{ApiClient, BlobCache, Cellar, Database, Installer, Linker, Store};

    use super::execute;
    use crate::ui::{Ui, UiOptions};

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

    #[tokio::test]
    async fn execute_separates_multiple_formula_sources_on_stdout() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer =
            test_installer(&root, &prefix, format!("{}/formula", mock_server.uri()));

        for name in ["foo", "bar"] {
            Mock::given(method("GET"))
                .and(path(format!("/formula/{name}.json")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "name": name,
                    "versions": { "stable": "1.0.0" },
                    "dependencies": [],
                    "bottle": { "stable": { "files": {} } },
                    "ruby_source_path": format!("{}/{name}.rb", mock_server.uri()),
                })))
                .mount(&mock_server)
                .await;

            Mock::given(method("GET"))
                .and(path(format!("/{name}.rb")))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string(format!("class {} < Formula\nend\n", name.to_uppercase())),
                )
                .mount(&mock_server)
                .await;
        }

        let (mut ui, out, err) = Ui::for_test(UiOptions::default());
        execute(
            &mut installer,
            vec!["foo".to_string(), "bar".to_string()],
            &mut ui,
        )
        .await
        .unwrap();

        assert_eq!(
            out.contents(),
            "class FOO < Formula\nend\n\nclass BAR < Formula\nend\n"
        );
        assert!(err.contents().is_empty());
    }
}
