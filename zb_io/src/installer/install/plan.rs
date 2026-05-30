use std::collections::BTreeMap;

use tracing::warn;
use zb_core::{BuildPlan, Error, Formula, InstallMethod, select_bottle};

use super::{InstallPlan, Installer, PlannedInstall};

impl Installer {
    pub async fn plan(&self, names: &[String]) -> Result<InstallPlan, Error> {
        self.plan_with_options(names, false).await
    }

    pub async fn plan_with_options(
        &self,
        names: &[String],
        build_from_source: bool,
    ) -> Result<InstallPlan, Error> {
        self.plan_with_behavior(names, build_from_source, false, false)
            .await
    }

    pub async fn plan_with_behavior(
        &self,
        names: &[String],
        build_from_source: bool,
        ignore_dependencies: bool,
        only_dependencies: bool,
    ) -> Result<InstallPlan, Error> {
        let formulas = self
            .fetch_all_formulas(names, build_from_source, ignore_dependencies)
            .await?;
        let ordered = zb_core::resolve_closure_with_options(names, &formulas, only_dependencies)?;

        let mut items = Vec::with_capacity(ordered.len());
        for install_name in ordered {
            let formula = formulas.get(&install_name).cloned().unwrap();
            let method = self.install_method_for_formula(&formula, build_from_source)?;

            if self.installed_formula_is_current(&install_name, &formula, &method) {
                continue;
            }

            items.push(PlannedInstall {
                install_name,
                formula,
                method,
            });
        }

        Ok(InstallPlan { items })
    }

    async fn fetch_all_formulas(
        &self,
        names: &[String],
        build_from_source: bool,
        ignore_dependencies: bool,
    ) -> Result<BTreeMap<String, Formula>, Error> {
        use std::collections::HashSet;

        let mut formulas = BTreeMap::new();
        let mut fetched: HashSet<String> = HashSet::new();
        let mut to_fetch: Vec<String> = names.to_vec();

        while !to_fetch.is_empty() {
            let batch: Vec<String> = to_fetch
                .drain(..)
                .filter(|n| !fetched.contains(n))
                .collect();

            if batch.is_empty() {
                break;
            }

            for n in &batch {
                fetched.insert(n.clone());
            }

            let futures: Vec<_> = batch
                .iter()
                .map(|n| self.api_client.get_formula(n))
                .collect();

            let results = futures::future::join_all(futures).await;

            for (i, result) in results.into_iter().enumerate() {
                let formula = match result {
                    Ok(f) => f,
                    Err(e) => return Err(e),
                };

                if select_bottle(&formula).is_err() && !formula.has_source_url() {
                    warn!(
                        formula = %formula.name,
                        "skipping formula with no bottle or source available for this platform"
                    );
                    continue;
                }

                let method = self.install_method_for_formula(&formula, build_from_source)?;
                if !ignore_dependencies
                    && !self.installed_formula_is_current(&batch[i], &formula, &method)
                {
                    for dep in &formula.dependencies {
                        if !fetched.contains(dep) && !to_fetch.contains(dep) {
                            to_fetch.push(dep.clone());
                        }
                    }
                }

                formulas.insert(batch[i].clone(), formula);
            }
        }

        Ok(formulas)
    }

    fn install_method_for_formula(
        &self,
        formula: &Formula,
        build_from_source: bool,
    ) -> Result<InstallMethod, Error> {
        if build_from_source {
            match BuildPlan::from_formula(formula, &self.prefix) {
                Some(plan) => Ok(InstallMethod::Source(plan)),
                None => select_bottle(formula)
                    .map(InstallMethod::Bottle)
                    .map_err(|_| Error::UnsupportedBottle {
                        name: formula.name.clone(),
                    }),
            }
        } else {
            match select_bottle(formula) {
                Ok(bottle) => Ok(InstallMethod::Bottle(bottle)),
                Err(_) => BuildPlan::from_formula(formula, &self.prefix).map_or_else(
                    || {
                        Err(Error::UnsupportedBottle {
                            name: formula.name.clone(),
                        })
                    },
                    |plan| Ok(InstallMethod::Source(plan)),
                ),
            }
        }
    }

    fn installed_formula_is_current(
        &self,
        install_name: &str,
        formula: &Formula,
        method: &InstallMethod,
    ) -> bool {
        self.db
            .get_installed(install_name)
            .is_some_and(|installed| {
                self.installed_keg_exists(&installed)
                    && installed.version == formula.effective_version()
                    && match method {
                        InstallMethod::Bottle(bottle) => installed.store_key == bottle.sha256,
                        InstallMethod::Source(_) => installed.store_key.starts_with("source:"),
                    }
            })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::cellar::Cellar;
    use crate::installer::install::test_support::*;
    use crate::network::api::ApiClient;
    use crate::storage::blob::BlobCache;
    use crate::storage::db::Database;
    use crate::storage::store::Store;
    use crate::{Installer, Linker};

    fn test_installer(
        root: &std::path::Path,
        prefix: &std::path::Path,
        api_url: String,
    ) -> Installer {
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client = ApiClient::with_base_url(api_url).unwrap();
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

    fn bottle_formula_json(name: &str, version: &str, sha256: &str) -> String {
        let tag = get_test_bottle_tag();
        format!(
            r#"{{
                "name": "{}",
                "versions": {{ "stable": "{}" }},
                "dependencies": [],
                "bottle": {{
                    "stable": {{
                        "files": {{
                            "{}": {{
                                "url": "https://example.com/{}.bottle.tar.gz",
                                "sha256": "{}"
                            }}
                        }}
                    }}
                }}
            }}"#,
            name, version, tag, name, sha256
        )
    }

    fn source_formula_json(name: &str, version: &str) -> String {
        format!(
            r#"{{
                "name": "{}",
                "versions": {{ "stable": "{}" }},
                "dependencies": [],
                "urls": {{
                    "stable": {{
                        "url": "https://example.com/{}-{}.tar.gz",
                        "checksum": "source-checksum"
                    }}
                }},
                "ruby_source_path": "Formula/{}/{}.rb",
                "bottle": {{ "stable": {{ "files": {{}} }} }}
            }}"#,
            name,
            version,
            name,
            version,
            &name[..1],
            name
        )
    }

    async fn mount_formula(mock_server: &MockServer, name: &str, body: String) {
        Mock::given(method("GET"))
            .and(path(format!("/formula/{}.json", name)))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(mock_server)
            .await;
    }

    fn record_installed(installer: &mut Installer, name: &str, version: &str, store_key: &str) {
        let tx = installer.db.transaction().unwrap();
        tx.record_install(name, version, store_key).unwrap();
        tx.commit().unwrap();
    }

    fn create_installed_keg(installer: &Installer, name: &str, version: &str) {
        fs::create_dir_all(installer.keg_path(name, version)).unwrap();
    }

    #[tokio::test]
    async fn plans_tapped_formula_with_core_dependency() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let dep_bottle = create_bottle_tarball("go");
        let dep_sha = sha256_hex(&dep_bottle);
        let tag = get_test_bottle_tag();
        let dep_json = format!(
            r#"{{
                "name": "go",
                "versions": {{ "stable": "1.24.0" }},
                "dependencies": [],
                "bottle": {{
                    "stable": {{
                        "files": {{
                            "{}": {{
                                "url": "{}/bottles/go-1.24.0.{}.bottle.tar.gz",
                                "sha256": "{}"
                            }}
                        }}
                    }}
                }}
            }}"#,
            tag,
            mock_server.uri(),
            tag,
            dep_sha
        );

        Mock::given(method("GET"))
            .and(path("/formula/go.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&dep_json))
            .mount(&mock_server)
            .await;

        let tap_formula_rb = format!(
            r#"
class Terraform < Formula
  version "1.10.0"
  depends_on "go"
  bottle do
    root_url "{}/ghcr/hashicorp/tap"
    sha256 {}: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  end
end
"#,
            mock_server.uri(),
            tag
        );

        Mock::given(method("GET"))
            .and(path("/hashicorp/homebrew-tap/main/Formula/terraform.rb"))
            .respond_with(ResponseTemplate::new(200).set_body_string(tap_formula_rb))
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client = ApiClient::with_base_url(format!("{}/formula", mock_server.uri()))
            .unwrap()
            .with_tap_raw_base_url(mock_server.uri());
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
            prefix.to_path_buf(),
            root.join("locks"),
        );
        let plan = installer
            .plan(&["hashicorp/tap/terraform".to_string()])
            .await
            .unwrap();

        let planned_names: Vec<String> = plan
            .items
            .iter()
            .map(|item| item.formula.name.clone())
            .collect();
        assert!(planned_names.contains(&"terraform".to_string()));
        assert!(planned_names.contains(&"go".to_string()));
    }

    #[tokio::test]
    async fn falls_back_to_source_when_no_bottle() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let formula_json = r#"{
            "name": "nobottle",
            "versions": { "stable": "1.0.0" },
            "dependencies": [],
            "build_dependencies": ["pkgconf"],
            "urls": {
                "stable": {
                    "url": "https://example.com/nobottle-1.0.0.tar.gz",
                    "checksum": "abc123"
                }
            },
            "ruby_source_path": "Formula/n/nobottle.rb",
            "bottle": { "stable": { "files": {} } }
        }"#;

        Mock::given(method("GET"))
            .and(path("/formula/nobottle.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(formula_json))
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
            prefix.clone(),
            root.join("locks"),
        );

        let plan = installer.plan(&["nobottle".to_string()]).await.unwrap();

        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].formula.name, "nobottle");
        assert!(matches!(
            plan.items[0].method,
            zb_core::InstallMethod::Source(_)
        ));

        if let zb_core::InstallMethod::Source(ref bp) = plan.items[0].method {
            assert_eq!(bp.source_url, "https://example.com/nobottle-1.0.0.tar.gz");
            assert_eq!(bp.formula_name, "nobottle");
            assert_eq!(bp.build_dependencies, vec!["pkgconf"]);
        }
    }

    #[tokio::test]
    async fn prefers_bottle_over_source() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let tag = get_test_bottle_tag();
        let formula_json = format!(
            r#"{{
                "name": "hasboth",
                "versions": {{ "stable": "2.0.0" }},
                "dependencies": [],
                "urls": {{
                    "stable": {{
                        "url": "https://example.com/hasboth-2.0.0.tar.gz",
                        "checksum": "def456"
                    }}
                }},
                "ruby_source_path": "Formula/h/hasboth.rb",
                "bottle": {{
                    "stable": {{
                        "files": {{
                            "{}": {{
                                "url": "https://example.com/hasboth.bottle.tar.gz",
                                "sha256": "aabbccdd"
                            }}
                        }}
                    }}
                }}
            }}"#,
            tag
        );

        Mock::given(method("GET"))
            .and(path("/formula/hasboth.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&formula_json))
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
            prefix.clone(),
            root.join("locks"),
        );

        let plan = installer.plan(&["hasboth".to_string()]).await.unwrap();

        assert_eq!(plan.items.len(), 1);
        assert!(matches!(
            plan.items[0].method,
            zb_core::InstallMethod::Bottle(_)
        ));
    }

    #[tokio::test]
    async fn skips_installed_bottle_when_version_and_store_key_match() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();
        let sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        mount_formula(
            &mock_server,
            "installed",
            bottle_formula_json("installed", "1.0.0", sha),
        )
        .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer =
            test_installer(&root, &prefix, format!("{}/formula", mock_server.uri()));
        record_installed(&mut installer, "installed", "1.0.0", sha);
        create_installed_keg(&installer, "installed", "1.0.0");

        let plan = installer.plan(&["installed".to_string()]).await.unwrap();

        assert!(plan.items.is_empty());
    }

    #[tokio::test]
    async fn skips_installed_bottle_without_fetching_dependencies() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();
        let tag = get_test_bottle_tag();
        let sha = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        let formula_json = format!(
            r#"{{
                "name": "installed",
                "versions": {{ "stable": "1.0.0" }},
                "dependencies": ["slowdep"],
                "bottle": {{
                    "stable": {{
                        "files": {{
                            "{}": {{
                                "url": "https://example.com/installed.bottle.tar.gz",
                                "sha256": "{}"
                            }}
                        }}
                    }}
                }}
            }}"#,
            tag, sha
        );

        mount_formula(&mock_server, "installed", formula_json).await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer =
            test_installer(&root, &prefix, format!("{}/formula", mock_server.uri()));
        record_installed(&mut installer, "installed", "1.0.0", sha);
        create_installed_keg(&installer, "installed", "1.0.0");

        let plan = installer.plan(&["installed".to_string()]).await.unwrap();

        assert!(plan.items.is_empty());
        let requests = mock_server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url.path(), "/formula/installed.json");
    }

    #[tokio::test]
    async fn skips_installed_source_build_when_version_matches() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();
        mount_formula(
            &mock_server,
            "sourceonly",
            source_formula_json("sourceonly", "1.0.0"),
        )
        .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer =
            test_installer(&root, &prefix, format!("{}/formula", mock_server.uri()));
        record_installed(
            &mut installer,
            "sourceonly",
            "1.0.0",
            "source:sourceonly:1.0.0",
        );
        create_installed_keg(&installer, "sourceonly", "1.0.0");

        let plan = installer.plan(&["sourceonly".to_string()]).await.unwrap();

        assert!(plan.items.is_empty());
    }

    #[tokio::test]
    async fn replans_when_installed_version_differs() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();
        let sha = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        mount_formula(
            &mock_server,
            "updatable",
            bottle_formula_json("updatable", "1.0.0", sha),
        )
        .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer =
            test_installer(&root, &prefix, format!("{}/formula", mock_server.uri()));
        record_installed(&mut installer, "updatable", "0.9.0", sha);
        create_installed_keg(&installer, "updatable", "0.9.0");

        let plan = installer.plan(&["updatable".to_string()]).await.unwrap();

        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].install_name, "updatable");
    }

    #[tokio::test]
    async fn replans_when_installed_store_key_differs() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();
        let sha = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        mount_formula(
            &mock_server,
            "changed",
            bottle_formula_json("changed", "1.0.0", sha),
        )
        .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer =
            test_installer(&root, &prefix, format!("{}/formula", mock_server.uri()));
        record_installed(&mut installer, "changed", "1.0.0", "old-sha");
        create_installed_keg(&installer, "changed", "1.0.0");

        let plan = installer.plan(&["changed".to_string()]).await.unwrap();

        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].install_name, "changed");
    }

    #[tokio::test]
    async fn replans_when_installed_keg_is_missing() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();
        let sha = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
        mount_formula(
            &mock_server,
            "stale",
            bottle_formula_json("stale", "1.0.0", sha),
        )
        .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        let mut installer =
            test_installer(&root, &prefix, format!("{}/formula", mock_server.uri()));
        record_installed(&mut installer, "stale", "1.0.0", sha);

        let plan = installer.plan(&["stale".to_string()]).await.unwrap();

        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].install_name, "stale");
    }

    #[tokio::test]
    async fn errors_when_no_bottle_and_no_source() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let formula_json = r#"{
            "name": "nothing",
            "versions": { "stable": "1.0.0" },
            "dependencies": [],
            "bottle": { "stable": { "files": {} } }
        }"#;

        Mock::given(method("GET"))
            .and(path("/formula/nothing.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(formula_json))
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
            prefix.clone(),
            root.join("locks"),
        );

        let result = installer.plan(&["nothing".to_string()]).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            zb_core::Error::MissingFormula { .. }
        ));
    }
}
