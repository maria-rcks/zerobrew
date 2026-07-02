use console::style;

use crate::ui::Ui;

pub async fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
    include_build: bool,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    let multiple = formulas.len() > 1;
    for formula in formulas {
        let dependencies = installer
            .formula_dependencies(&formula, include_build)
            .await?;
        if multiple {
            ui.data(style(&formula).bold());
        }
        for dependency in dependencies {
            ui.data(dependency);
        }
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
    async fn execute_writes_dependencies_to_stdout_only() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer =
            test_installer(&root, &prefix, format!("{}/formula", mock_server.uri()));

        Mock::given(method("GET"))
            .and(path("/formula/root.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{
                    "name":"root",
                    "versions":{"stable":"1.0.0"},
                    "dependencies":["zlib"],
                    "build_dependencies":["pkgconf"],
                    "bottle":{"stable":{"files":{}}}
                }"#,
            ))
            .mount(&mock_server)
            .await;

        let (mut ui, out, err) = Ui::for_test(UiOptions::default());
        execute(&mut installer, vec!["root".to_string()], true, &mut ui)
            .await
            .unwrap();

        assert_eq!(out.contents(), "pkgconf\nzlib\n");
        assert!(err.contents().is_empty());
    }
}
