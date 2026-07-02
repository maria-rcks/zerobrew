use crate::ui::Ui;
use console::style;
use std::path::PathBuf;
use zb_io::Installer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    Auto,
    Formula,
    Cask,
}

pub fn normalize_formula_name(name: &str) -> Result<String, zb_core::Error> {
    normalize_package_name(name, PackageKind::Auto)
}

pub fn normalize_package_name(name: &str, kind: PackageKind) -> Result<String, zb_core::Error> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(zb_core::Error::MissingFormula {
            name: name.to_string(),
        });
    }

    if let Some(token) = trimmed.strip_prefix("cask:") {
        if matches!(kind, PackageKind::Formula) {
            return Err(zb_core::Error::InvalidArgument {
                message: format!("'{trimmed}' is a cask, but --formula was specified"),
            });
        }
        let token = token.trim();
        if token.is_empty() {
            return Err(zb_core::Error::InvalidArgument {
                message: "cask token cannot be empty".to_string(),
            });
        }
        return Ok(format!("cask:{token}"));
    }

    if let Some((tap, formula)) = trimmed.rsplit_once('/') {
        if formula.is_empty() {
            return Err(zb_core::Error::MissingFormula {
                name: trimmed.to_string(),
            });
        }

        if tap == "homebrew/core" {
            if matches!(kind, PackageKind::Cask) {
                return Ok(format!("cask:{formula}"));
            }
            return Ok(formula.to_string());
        }

        if tap == "homebrew/cask" {
            if matches!(kind, PackageKind::Formula) {
                return Err(zb_core::Error::InvalidArgument {
                    message: format!("'{trimmed}' is a cask, but --formula was specified"),
                });
            }
            return Ok(format!("cask:{formula}"));
        }

        if matches!(kind, PackageKind::Cask) {
            return Ok(format!("cask:{trimmed}"));
        }

        return Ok(trimmed.to_string());
    }

    if matches!(kind, PackageKind::Cask) {
        return Ok(format!("cask:{trimmed}"));
    }

    Ok(trimmed.to_string())
}

pub fn suggest_formula_matches(ui: &mut Ui, requested: &str, suggestions: &[String]) {
    if suggestions.is_empty() {
        return;
    }

    ui.error_blank_line();
    ui.error_hint(format!(
        "Formula '{}' was not found. Did you mean:",
        style(requested).bold().for_stderr()
    ));
    for suggestion in suggestions {
        ui.error_status(format!("      {}", style(suggestion).green().for_stderr()));
    }

    if let Some(top_suggestion) = suggestions.first() {
        ui.error_blank_line();
        ui.error_status("      Try installing the closest match with zerobrew:");
        ui.error_status(format!(
            "      {}",
            style(format!("zb install {top_suggestion}"))
                .cyan()
                .for_stderr()
        ));
    }
    ui.error_blank_line();
}

pub async fn suggest_missing_formula_matches(
    installer: &Installer,
    error: &zb_core::Error,
    ui: &mut Ui,
) -> bool {
    if let zb_core::Error::MissingFormula { name } = error {
        match installer.suggest_formulas(name, 3).await {
            Ok(suggestions) => suggest_formula_matches(ui, name, &suggestions),
            Err(lookup_error) => {
                tracing::debug!("failed to look up formula suggestions: {lookup_error}");
            }
        }
        return true;
    }

    false
}

pub fn suggest_homebrew(ui: &mut Ui, formula: &str, error: &zb_core::Error) {
    ui.error_blank_line();
    ui.error_note("This package can't be installed with zerobrew.");
    ui.error_status(format!("      Error: {error}"));
    ui.error_blank_line();

    // Error for Termux on android since homebrew
    // doesn't support bottles for this platform
    // details: https://github.com/lucasgelfond/zerobrew/pull/136
    if cfg!(target_os = "android") {
        ui.error_status(format!(
            "      {} {}",
            style(formula).yellow().bold().for_stderr(),
            style(
                "is not compatible with Termux - homebrew bottles are not available for Android."
            )
            .red()
            .bold()
            .for_stderr()
        ));
        ui.error_status(format!(
            "      {}",
            style("and cannot be installed on it.")
                .red()
                .bold()
                .for_stderr()
        ));
    } else {
        ui.error_status("      Try installing with Homebrew instead:");
        ui.error_status(format!(
            "      {}",
            style(format!("brew install {formula}")).cyan().for_stderr()
        ));
    }

    ui.error_blank_line();
}

pub fn get_root_path(cli_root: Option<PathBuf>) -> PathBuf {
    if let Some(root) = cli_root {
        return root;
    }

    if let Ok(env_root) = std::env::var("ZEROBREW_ROOT") {
        return PathBuf::from(env_root);
    }

    let legacy_root = PathBuf::from("/opt/zerobrew");
    if legacy_root.exists() {
        return legacy_root;
    }

    if cfg!(target_os = "macos") {
        legacy_root
    } else {
        let xdg_data_home = std::env::var("XDG_DATA_HOME")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var("HOME")
                    .map(|h| PathBuf::from(h).join(".local").join("share"))
                    .unwrap_or_else(|_| legacy_root.clone())
            });
        xdg_data_home.join("zerobrew")
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use zb_io::cellar::Cellar;
    use zb_io::network::ApiClient;
    use zb_io::storage::{BlobCache, Database, Store};
    use zb_io::{Installer, Linker};

    use super::{
        PackageKind, normalize_formula_name, normalize_package_name, suggest_formula_matches,
        suggest_homebrew, suggest_missing_formula_matches,
    };
    use crate::ui::{Ui, UiOptions};

    #[test]
    fn normalize_core_tap_formula() {
        assert_eq!(
            normalize_formula_name("homebrew/core/wget").unwrap(),
            "wget".to_string()
        );
    }

    #[test]
    fn normalize_external_tap_formula_keeps_full_name() {
        assert_eq!(
            normalize_formula_name("hashicorp/tap/terraform").unwrap(),
            "hashicorp/tap/terraform".to_string()
        );
    }

    #[test]
    fn normalize_external_tap_cask_with_cask_selection_prefixes_full_name() {
        assert_eq!(
            normalize_package_name("kamillobinski/thock/thock", PackageKind::Cask).unwrap(),
            "cask:kamillobinski/thock/thock".to_string()
        );
    }

    #[test]
    fn normalize_homebrew_cask_prefixes_token() {
        assert_eq!(
            normalize_formula_name("homebrew/cask/docker-desktop").unwrap(),
            "cask:docker-desktop".to_string()
        );
    }

    #[test]
    fn normalize_cask_selection_prefixes_plain_token() {
        assert_eq!(
            normalize_package_name("firefox", PackageKind::Cask).unwrap(),
            "cask:firefox".to_string()
        );
    }

    #[test]
    fn normalize_formula_selection_rejects_cask_token() {
        let err = normalize_package_name("cask:firefox", PackageKind::Formula).unwrap_err();
        assert!(matches!(err, zb_core::Error::InvalidArgument { .. }));
    }

    #[test]
    fn normalize_rejects_blank_formula() {
        let err = normalize_formula_name(" \t ").unwrap_err();
        assert!(matches!(err, zb_core::Error::MissingFormula { .. }));
    }

    #[test]
    fn normalize_rejects_empty_cask_token() {
        let err = normalize_formula_name("cask:   ").unwrap_err();
        assert!(matches!(err, zb_core::Error::InvalidArgument { .. }));
    }

    #[test]
    fn normalize_trims_cask_token() {
        assert_eq!(
            normalize_formula_name(" cask:docker-desktop ").unwrap(),
            "cask:docker-desktop".to_string()
        );
    }

    #[test]
    fn suggest_formula_matches_renders_list_on_stderr() {
        let (mut ui, out, err) = Ui::for_test(UiOptions::default());

        suggest_formula_matches(
            &mut ui,
            "pythn",
            &["python".to_string(), "pytest".to_string()],
        );

        let rendered = err.contents();
        assert!(rendered.contains("Did you mean"));
        assert!(rendered.contains("python"));
        assert!(rendered.contains("pytest"));
        assert!(rendered.contains("zb install python"));
        assert!(out.contents().is_empty(), "hints must not pollute stdout");
    }

    #[test]
    fn fatal_suggestions_survive_quiet_mode() {
        let (mut ui, out, err) = Ui::for_test(UiOptions {
            quiet: true,
            ..Default::default()
        });

        ui.note("ordinary note");
        suggest_formula_matches(&mut ui, "pythn", &["python".to_string()]);
        suggest_homebrew(
            &mut ui,
            "unportable",
            &zb_core::Error::UnsupportedBottle {
                name: "unportable".to_string(),
            },
        );

        let rendered = err.contents();
        assert!(rendered.contains("Did you mean"));
        assert!(rendered.contains("zb install python"));
        assert!(rendered.contains("This package can't be installed with zerobrew."));
        assert!(rendered.contains("brew install unportable"));
        assert!(!rendered.contains("ordinary note"));
        assert!(out.contents().is_empty(), "hints must not pollute stdout");
    }

    #[test]
    fn suggest_formula_matches_prints_nothing_for_empty_input() {
        let (mut ui, out, err) = Ui::for_test(UiOptions::default());

        suggest_formula_matches(&mut ui, "pythn", &[]);

        assert!(out.contents().is_empty());
        assert!(err.contents().is_empty());
    }

    #[tokio::test]
    async fn suggest_missing_formula_matches_fetches_related_suggestions() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        Mock::given(method("GET"))
            .and(path("/formula.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"[
                    {"name":"python"},
                    {"name":"pytest"}
                ]"#,
            ))
            .expect(1)
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client =
            ApiClient::with_base_url(format!("{}/formula", mock_server.uri())).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();
        let installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix,
            root.join("locks"),
        );

        let error = zb_core::Error::MissingFormula {
            name: "pythn".to_string(),
        };

        let (mut ui, _out, err) = Ui::for_test(UiOptions::default());
        assert!(suggest_missing_formula_matches(&installer, &error, &mut ui).await);
        assert!(err.contents().contains("Did you mean"));
    }

    #[tokio::test]
    async fn suggest_missing_formula_matches_returns_false_for_non_missing_errors() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client = ApiClient::new();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();
        let installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix,
            root.join("locks"),
        );

        let error = zb_core::Error::InvalidArgument {
            message: "bad formula".to_string(),
        };

        let (mut ui, _out, _err) = Ui::for_test(UiOptions::default());
        assert!(!suggest_missing_formula_matches(&installer, &error, &mut ui).await);
    }
}
