use serde_json::Value;
use zb_core::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaskBinary {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaskApp {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaskFont {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaskPkg {
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaskSuite {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaskGenericArtifact {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaskAppImage {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaskInstaller {
    pub kind: CaskInstallerKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaskInstallerKind {
    Manual {
        path: String,
    },
    Script {
        executable: String,
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCask {
    pub install_name: String,
    pub token: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub binaries: Vec<CaskBinary>,
    pub apps: Vec<CaskApp>,
    pub fonts: Vec<CaskFont>,
    pub pkgs: Vec<CaskPkg>,
    pub suites: Vec<CaskSuite>,
    pub generic_artifacts: Vec<CaskGenericArtifact>,
    pub app_images: Vec<CaskAppImage>,
    pub installers: Vec<CaskInstaller>,
    pub stage_only: bool,
    pub depends_on_formulas: Vec<String>,
    pub depends_on_casks: Vec<String>,
}

pub fn resolve_cask(token: &str, cask: &Value) -> Result<ResolvedCask, Error> {
    let mut url = required_string(cask, "url")?;
    let mut sha256 = required_string(cask, "sha256")?;
    let version = required_string(cask, "version")?;

    if let Some(variation) = select_platform_variation(cask) {
        if let Some(variation_url) = variation.get("url").and_then(Value::as_str) {
            url = variation_url.to_string();
        }
        if let Some(variation_sha) = variation.get("sha256").and_then(Value::as_str) {
            sha256 = variation_sha.to_string();
        }
    }

    if sha256 == "no_check" {
        return Err(Error::InvalidArgument {
            message: format!("cask '{token}' uses an unsupported checksum mode: no_check"),
        });
    }

    let binaries = parse_binary_artifacts(cask)?;
    let apps = parse_app_artifacts(cask)?;
    let fonts = parse_font_artifacts(cask)?;
    let pkgs = parse_pkg_artifacts(cask)?;
    let suites = parse_suite_artifacts(cask)?;
    let generic_artifacts = parse_generic_artifacts(cask)?;
    let app_images = parse_appimage_artifacts(cask)?;
    let installers = parse_installer_artifacts(cask)?;
    let stage_only = parse_stage_only_artifact(cask)?;
    let depends_on_formulas = parse_depends_on_values(cask, "formula")?;
    let depends_on_casks = parse_depends_on_values(cask, "cask")?;
    if binaries.is_empty()
        && apps.is_empty()
        && fonts.is_empty()
        && pkgs.is_empty()
        && suites.is_empty()
        && generic_artifacts.is_empty()
        && app_images.is_empty()
        && installers.is_empty()
        && !stage_only
    {
        let found = artifact_types(cask);
        return Err(Error::InvalidArgument {
            message: format!(
                "cask '{token}' has no supported artifacts (found: {found}); \
                 only casks with 'binary', 'app', 'font', 'pkg', 'suite', 'artifact', 'appimage', 'installer', and 'stage_only' artifacts are currently supported"
            ),
        });
    }

    Ok(ResolvedCask {
        install_name: format!("cask:{token}"),
        token: token.to_string(),
        version,
        url,
        sha256,
        binaries,
        apps,
        fonts,
        pkgs,
        suites,
        generic_artifacts,
        app_images,
        installers,
        stage_only,
        depends_on_formulas,
        depends_on_casks,
    })
}

fn required_string(value: &Value, field: &str) -> Result<String, Error> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Error::InvalidArgument {
            message: format!("failed to parse cask JSON: missing string field '{field}'"),
        })
}

fn select_platform_variation(cask: &Value) -> Option<&Value> {
    let variations = cask.get("variations")?;
    preferred_variation_keys()
        .iter()
        .find_map(|key| variations.get(key))
}

fn preferred_variation_keys() -> &'static [&'static str] {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        &["x86_64_linux", "arm64_linux"]
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        &["arm64_linux", "x86_64_linux"]
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        &[
            "arm64_tahoe",
            "arm64_sequoia",
            "arm64_sonoma",
            "arm64_ventura",
            "arm64_monterey",
            "arm64_big_sur",
        ]
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        &[
            "tahoe", "sequoia", "sonoma", "ventura", "monterey", "big_sur", "catalina",
        ]
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        &[]
    }
}

fn artifact_types(cask: &Value) -> String {
    let types: Vec<&str> = cask
        .get("artifacts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| a.as_object())
        .flat_map(|obj| obj.keys())
        .map(String::as_str)
        .collect();

    if types.is_empty() {
        "none".to_string()
    } else {
        types.join(", ")
    }
}

fn parse_binary_artifacts(cask: &Value) -> Result<Vec<CaskBinary>, Error> {
    let mut binaries = Vec::new();
    let artifacts = cask
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidArgument {
            message: "failed to parse cask JSON: missing artifacts array".to_string(),
        })?;

    for artifact in artifacts {
        let Some(value) = artifact.get("binary") else {
            continue;
        };

        for entry in artifact_entries(value)? {
            let (source, target) = parse_binary_entry(entry)?;
            binaries.push(CaskBinary { source, target });
        }
    }

    Ok(binaries)
}

fn parse_app_artifacts(cask: &Value) -> Result<Vec<CaskApp>, Error> {
    let mut apps = Vec::new();
    let artifacts = cask
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidArgument {
            message: "failed to parse cask JSON: missing artifacts array".to_string(),
        })?;

    for artifact in artifacts {
        let Some(value) = artifact.get("app") else {
            continue;
        };

        for entry in artifact_entries(value)? {
            let (source, target) = parse_app_entry(entry)?;
            apps.push(CaskApp { source, target });
        }
    }

    Ok(apps)
}

fn parse_font_artifacts(cask: &Value) -> Result<Vec<CaskFont>, Error> {
    let mut fonts = Vec::new();
    let artifacts = cask
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidArgument {
            message: "failed to parse cask JSON: missing artifacts array".to_string(),
        })?;

    for artifact in artifacts {
        let Some(value) = artifact.get("font") else {
            continue;
        };

        for entry in artifact_entries(value)? {
            let (source, target) = parse_font_entry(entry)?;
            fonts.push(CaskFont { source, target });
        }
    }

    Ok(fonts)
}

fn parse_pkg_artifacts(cask: &Value) -> Result<Vec<CaskPkg>, Error> {
    let mut pkgs = Vec::new();
    let artifacts = cask
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidArgument {
            message: "failed to parse cask JSON: missing artifacts array".to_string(),
        })?;

    for artifact in artifacts {
        let Some(value) = artifact.get("pkg") else {
            continue;
        };

        for entry in artifact_entries(value)? {
            pkgs.push(CaskPkg {
                source: parse_pkg_entry(entry)?,
            });
        }
    }

    Ok(pkgs)
}

fn parse_suite_artifacts(cask: &Value) -> Result<Vec<CaskSuite>, Error> {
    let mut suites = Vec::new();
    let artifacts = cask
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidArgument {
            message: "failed to parse cask JSON: missing artifacts array".to_string(),
        })?;

    for artifact in artifacts {
        let Some(value) = artifact.get("suite") else {
            continue;
        };

        for entry in artifact_entries(value)? {
            let (source, target) = parse_moved_entry(entry, "suite", true)?;
            suites.push(CaskSuite { source, target });
        }
    }

    Ok(suites)
}

fn parse_generic_artifacts(cask: &Value) -> Result<Vec<CaskGenericArtifact>, Error> {
    let mut generic_artifacts = Vec::new();
    let artifacts = cask
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidArgument {
            message: "failed to parse cask JSON: missing artifacts array".to_string(),
        })?;

    for artifact in artifacts {
        let Some(value) = artifact.get("artifact") else {
            continue;
        };

        for entry in artifact_entries(value)? {
            let (source, target) = parse_moved_entry(entry, "artifact", false)?;
            generic_artifacts.push(CaskGenericArtifact { source, target });
        }
    }

    Ok(generic_artifacts)
}

fn parse_appimage_artifacts(cask: &Value) -> Result<Vec<CaskAppImage>, Error> {
    let mut app_images = Vec::new();
    let artifacts = cask
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidArgument {
            message: "failed to parse cask JSON: missing artifacts array".to_string(),
        })?;

    for artifact in artifacts {
        let Some(value) = artifact.get("appimage") else {
            continue;
        };

        for entry in artifact_entries(value)? {
            let (source, target) = parse_moved_entry(entry, "appimage", true)?;
            app_images.push(CaskAppImage { source, target });
        }
    }

    Ok(app_images)
}

fn parse_installer_artifacts(cask: &Value) -> Result<Vec<CaskInstaller>, Error> {
    let mut installers = Vec::new();
    let artifacts = cask
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidArgument {
            message: "failed to parse cask JSON: missing artifacts array".to_string(),
        })?;

    for artifact in artifacts {
        let Some(value) = artifact.get("installer") else {
            continue;
        };

        for entry in artifact_entries(value)? {
            installers.push(parse_installer_entry(entry)?);
        }
    }

    Ok(installers)
}

fn parse_stage_only_artifact(cask: &Value) -> Result<bool, Error> {
    let artifacts = cask
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidArgument {
            message: "failed to parse cask JSON: missing artifacts array".to_string(),
        })?;

    Ok(artifacts.iter().any(|artifact| {
        artifact
            .get("stage_only")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }))
}

fn parse_depends_on_values(cask: &Value, key: &str) -> Result<Vec<String>, Error> {
    let Some(value) = cask
        .get("depends_on")
        .and_then(|depends_on| depends_on.get(key))
    else {
        return Ok(Vec::new());
    };

    if let Some(dep) = value.as_str() {
        return Ok(vec![dep.to_string()]);
    }

    let Some(deps) = value.as_array() else {
        return Err(Error::InvalidArgument {
            message: format!("unsupported cask depends_on {key} shape"),
        });
    };

    Ok(deps
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect())
}

fn artifact_entries(value: &Value) -> Result<Vec<&Value>, Error> {
    let Some(entries) = value.as_array() else {
        return Ok(vec![value]);
    };

    if entries.first().is_some_and(Value::is_string) {
        return Ok(vec![value]);
    }

    Ok(entries.iter().collect())
}

fn parse_binary_entry(entry: &Value) -> Result<(String, String), Error> {
    if let Some(path) = entry.as_str() {
        return Ok((path.to_string(), basename(path)?));
    }

    let array = entry.as_array().ok_or_else(|| Error::InvalidArgument {
        message: "unsupported cask binary artifact shape".to_string(),
    })?;
    let source = array
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidArgument {
            message: "unsupported cask binary source".to_string(),
        })?;

    let target = array
        .get(1)
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("target"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| basename(source).unwrap_or_else(|_| source.to_string()));

    validate_relative_target(&target, "binary")?;

    Ok((source.to_string(), target))
}

fn parse_app_entry(entry: &Value) -> Result<(String, String), Error> {
    if let Some(path) = entry.as_str() {
        return Ok((path.to_string(), basename(path)?));
    }

    let array = entry.as_array().ok_or_else(|| Error::InvalidArgument {
        message: "unsupported cask app artifact shape".to_string(),
    })?;
    let source = array
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidArgument {
            message: "unsupported cask app source".to_string(),
        })?;

    let target = array
        .get(1)
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("target"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| basename(source).unwrap_or_else(|_| source.to_string()));

    validate_relative_target(&target, "app")?;
    Ok((source.to_string(), target))
}

fn parse_font_entry(entry: &Value) -> Result<(String, String), Error> {
    if let Some(path) = entry.as_str() {
        return Ok((path.to_string(), basename(path)?));
    }

    let array = entry.as_array().ok_or_else(|| Error::InvalidArgument {
        message: "unsupported cask font artifact shape".to_string(),
    })?;
    let source = array
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidArgument {
            message: "unsupported cask font source".to_string(),
        })?;

    let target = array
        .get(1)
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("target"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| basename(source).unwrap_or_else(|_| source.to_string()));

    validate_relative_target(&target, "font")?;
    Ok((source.to_string(), target))
}

fn parse_pkg_entry(entry: &Value) -> Result<String, Error> {
    if let Some(path) = entry.as_str() {
        return Ok(path.to_string());
    }

    let array = entry.as_array().ok_or_else(|| Error::InvalidArgument {
        message: "unsupported cask pkg artifact shape".to_string(),
    })?;
    array
        .first()
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Error::InvalidArgument {
            message: "unsupported cask pkg source".to_string(),
        })
}

fn parse_moved_entry(
    entry: &Value,
    artifact_kind: &str,
    default_target: bool,
) -> Result<(String, String), Error> {
    if let Some(path) = entry.as_str() {
        if default_target {
            return Ok((path.to_string(), basename(path)?));
        }

        return Err(Error::InvalidArgument {
            message: format!("cask {artifact_kind} artifact requires a target"),
        });
    }

    let array = entry.as_array().ok_or_else(|| Error::InvalidArgument {
        message: format!("unsupported cask {artifact_kind} artifact shape"),
    })?;
    let source = array
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidArgument {
            message: format!("unsupported cask {artifact_kind} source"),
        })?;

    let target = array
        .get(1)
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("target"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| default_target.then(|| basename(source).unwrap_or_else(|_| source.to_string())))
        .ok_or_else(|| Error::InvalidArgument {
            message: format!("cask {artifact_kind} artifact requires a target"),
        })?;

    if artifact_kind != "artifact" {
        validate_relative_target(&target, artifact_kind)?;
    }

    Ok((source.to_string(), target))
}

fn parse_installer_entry(entry: &Value) -> Result<CaskInstaller, Error> {
    let object = entry.as_object().ok_or_else(|| Error::InvalidArgument {
        message: "unsupported cask installer artifact shape".to_string(),
    })?;

    if let Some(manual) = object.get("manual").and_then(Value::as_str) {
        return Ok(CaskInstaller {
            kind: CaskInstallerKind::Manual {
                path: manual.to_string(),
            },
        });
    }

    if let Some(script) = object.get("script").and_then(Value::as_object) {
        let executable = script
            .get("executable")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::InvalidArgument {
                message: "unsupported cask installer script without executable".to_string(),
            })?;
        let args = script
            .get("args")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        return Ok(CaskInstaller {
            kind: CaskInstallerKind::Script {
                executable: executable.to_string(),
                args,
            },
        });
    }

    Err(Error::InvalidArgument {
        message: "unsupported cask installer artifact".to_string(),
    })
}

fn validate_relative_target(target: &str, artifact_kind: &str) -> Result<(), Error> {
    if target.contains('/') || target.contains('$') || target.contains('~') {
        return Err(Error::InvalidArgument {
            message: format!("unsupported cask {artifact_kind} target path '{target}'"),
        });
    }
    let mut components = std::path::Path::new(target).components();
    let Some(std::path::Component::Normal(name)) = components.next() else {
        return Err(Error::InvalidArgument {
            message: format!("unsupported cask {artifact_kind} target path '{target}'"),
        });
    };
    if components.next().is_some() || name.is_empty() {
        return Err(Error::InvalidArgument {
            message: format!("unsupported cask {artifact_kind} target path '{target}'"),
        });
    }
    Ok(())
}

fn basename(path: &str) -> Result<String, Error> {
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::InvalidArgument {
            message: format!("invalid cask binary path '{path}'"),
        })?;
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_cask_uses_platform_variation_url_and_sha() {
        let cask = serde_json::json!({
            "token": "test",
            "version": "1.0.0",
            "url": "https://example.com/darwin.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [{ "binary": [["op"]] }],
            "variations": {
                "x86_64_linux": {
                    "url": "https://example.com/linux.zip",
                    "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                }
            }
        });

        let _resolved = resolve_cask("test", &cask).unwrap();
        #[cfg(target_os = "linux")]
        {
            assert_eq!(_resolved.url, "https://example.com/linux.zip");
            assert_eq!(
                _resolved.sha256,
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            );
        }
    }

    #[test]
    fn resolve_cask_parses_binary_targets() {
        let cask = serde_json::json!({
            "token": "test",
            "version": "1.0.0",
            "url": "https://example.com/test.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [{
                "binary": [
                    ["bin/tool"],
                    ["bin/tool2", {"target": "tool-two"}]
                ]
            }]
        });

        let resolved = resolve_cask("test", &cask).unwrap();
        assert_eq!(resolved.binaries.len(), 2);
        assert_eq!(resolved.binaries[0].target, "tool");
        assert_eq!(resolved.binaries[1].target, "tool-two");
        assert!(resolved.apps.is_empty());
    }

    #[test]
    fn resolve_cask_parses_homebrew_arg_array_artifact() {
        let cask = serde_json::json!({
            "token": "omniwm",
            "version": "1.0.0",
            "url": "https://example.com/omniwm.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [
                { "app": ["OmniWM.app"] },
                {
                    "binary": [
                        "/Applications/OmniWM.app/Contents/MacOS/omniwmctl",
                        {"target": "omniwmctl"}
                    ]
                }
            ]
        });

        let resolved = resolve_cask("omniwm", &cask).unwrap();
        assert_eq!(resolved.apps.len(), 1);
        assert_eq!(resolved.binaries.len(), 1);
        assert_eq!(
            resolved.binaries[0].source,
            "/Applications/OmniWM.app/Contents/MacOS/omniwmctl"
        );
        assert_eq!(resolved.binaries[0].target, "omniwmctl");
    }

    #[test]
    fn resolve_cask_parses_app_targets() {
        let cask = serde_json::json!({
            "token": "test",
            "version": "1.0.0",
            "url": "https://example.com/test.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [
                {
                    "binary": [["bin/tool"]],
                    "app": [
                        ["Test.app"],
                        ["Subdir/Other.app", {"target": "Renamed.app"}]
                    ]
                }
            ]
        });

        let resolved = resolve_cask("test", &cask).unwrap();
        assert_eq!(resolved.apps.len(), 2);
        assert_eq!(resolved.apps[0].target, "Test.app");
        assert_eq!(resolved.apps[1].source, "Subdir/Other.app");
        assert_eq!(resolved.apps[1].target, "Renamed.app");
    }

    #[test]
    fn resolve_cask_parses_font_targets() {
        let cask = serde_json::json!({
            "token": "font-test",
            "version": "1.0.0",
            "url": "https://example.com/font.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [
                {
                    "font": [
                        ["Test-Regular.otf"],
                        ["Subdir/Test-Bold.otf", {"target": "Renamed-Bold.otf"}]
                    ]
                }
            ]
        });

        let resolved = resolve_cask("font-test", &cask).unwrap();
        assert!(resolved.binaries.is_empty());
        assert!(resolved.apps.is_empty());
        assert_eq!(resolved.fonts.len(), 2);
        assert_eq!(resolved.fonts[0].target, "Test-Regular.otf");
        assert_eq!(resolved.fonts[1].source, "Subdir/Test-Bold.otf");
        assert_eq!(resolved.fonts[1].target, "Renamed-Bold.otf");
    }

    #[test]
    fn resolve_cask_rejects_dot_segment_targets() {
        for target in [".", "..", ""] {
            let cask = serde_json::json!({
                "token": "test",
                "version": "1.0.0",
                "url": "https://example.com/test.zip",
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "artifacts": [{ "app": [["Test.app", { "target": target }]] }]
            });

            let err = resolve_cask("test", &cask).unwrap_err();
            assert!(matches!(err, Error::InvalidArgument { .. }));
        }
    }

    #[test]
    fn resolve_cask_parses_pkg_artifacts() {
        let cask = serde_json::json!({
            "token": "pkg-test",
            "version": "1.0.0",
            "url": "https://example.com/pkg.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [
                { "pkg": ["Install Test.pkg"] },
                { "pkg": [["Nested/Other.pkg"]] }
            ]
        });

        let resolved = resolve_cask("pkg-test", &cask).unwrap();
        assert!(resolved.binaries.is_empty());
        assert!(resolved.apps.is_empty());
        assert!(resolved.fonts.is_empty());
        assert_eq!(resolved.pkgs.len(), 2);
        assert_eq!(resolved.pkgs[0].source, "Install Test.pkg");
        assert_eq!(resolved.pkgs[1].source, "Nested/Other.pkg");
    }

    #[test]
    fn resolve_cask_parses_suite_artifacts() {
        let cask = serde_json::json!({
            "token": "suite-test",
            "version": "1.0.0",
            "url": "https://example.com/suite.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [
                { "suite": ["Office"] },
                { "suite": [["Tools", {"target": "Dev Tools"}]] }
            ]
        });

        let resolved = resolve_cask("suite-test", &cask).unwrap();
        assert_eq!(resolved.suites.len(), 2);
        assert_eq!(resolved.suites[0].target, "Office");
        assert_eq!(resolved.suites[1].source, "Tools");
        assert_eq!(resolved.suites[1].target, "Dev Tools");
    }

    #[test]
    fn resolve_cask_parses_generic_artifact_targets() {
        let cask = serde_json::json!({
            "token": "artifact-test",
            "version": "1.0.0",
            "url": "https://example.com/artifact.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [
                { "artifact": [["config/example.conf", {"target": "etc/example.conf"}]] }
            ]
        });

        let resolved = resolve_cask("artifact-test", &cask).unwrap();
        assert_eq!(resolved.generic_artifacts.len(), 1);
        assert_eq!(resolved.generic_artifacts[0].source, "config/example.conf");
        assert_eq!(resolved.generic_artifacts[0].target, "etc/example.conf");
    }

    #[test]
    fn resolve_cask_parses_appimage_artifacts() {
        let cask = serde_json::json!({
            "token": "appimage-test",
            "version": "1.0.0",
            "url": "https://example.com/appimage.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [
                { "appimage": [["Demo.AppImage", {"target": "Demo"}]] }
            ]
        });

        let resolved = resolve_cask("appimage-test", &cask).unwrap();
        assert_eq!(resolved.app_images.len(), 1);
        assert_eq!(resolved.app_images[0].source, "Demo.AppImage");
        assert_eq!(resolved.app_images[0].target, "Demo");
    }

    #[test]
    fn resolve_cask_parses_depends_on_formula_and_cask() {
        let cask = serde_json::json!({
            "token": "dependency-test",
            "version": "1.0.0",
            "url": "https://example.com/dependency.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "depends_on": {
                "formula": ["openssl@3", "python@3.13"],
                "cask": "xquartz"
            },
            "artifacts": [
                { "binary": ["bin/tool"] }
            ]
        });

        let resolved = resolve_cask("dependency-test", &cask).unwrap();
        assert_eq!(
            resolved.depends_on_formulas,
            vec!["openssl@3".to_string(), "python@3.13".to_string()]
        );
        assert_eq!(resolved.depends_on_casks, vec!["xquartz".to_string()]);
    }

    #[test]
    fn resolve_cask_parses_installer_artifacts() {
        let cask = serde_json::json!({
            "token": "installer-test",
            "version": "1.0.0",
            "url": "https://example.com/installer.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [
                { "installer": [{ "manual": "Install.app" }] },
                { "installer": [{ "script": { "executable": "install.sh", "args": ["--prefix", "$HOMEBREW_PREFIX"] } }] }
            ]
        });

        let resolved = resolve_cask("installer-test", &cask).unwrap();
        assert_eq!(resolved.installers.len(), 2);
        assert_eq!(
            resolved.installers[0].kind,
            CaskInstallerKind::Manual {
                path: "Install.app".to_string()
            }
        );
        assert_eq!(
            resolved.installers[1].kind,
            CaskInstallerKind::Script {
                executable: "install.sh".to_string(),
                args: vec!["--prefix".to_string(), "$HOMEBREW_PREFIX".to_string()]
            }
        );
    }

    #[test]
    fn resolve_cask_accepts_stage_only_artifact() {
        let cask = serde_json::json!({
            "token": "stage-only",
            "version": "1.0.0",
            "url": "https://example.com/stage-only.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [
                { "stage_only": [true] }
            ]
        });

        let resolved = resolve_cask("stage-only", &cask).unwrap();
        assert!(resolved.stage_only);
        assert!(resolved.binaries.is_empty());
        assert!(resolved.apps.is_empty());
        assert!(resolved.fonts.is_empty());
        assert!(resolved.pkgs.is_empty());
    }

    #[test]
    fn resolve_cask_missing_required_field_is_invalid_argument() {
        let cask = serde_json::json!({
            "token": "test",
            "version": "1.0.0",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [{ "binary": [["op"]] }]
        });

        let err = resolve_cask("test", &cask).unwrap_err();
        assert!(matches!(err, Error::InvalidArgument { .. }));
    }

    #[test]
    fn resolve_cask_missing_artifacts_array_is_invalid_argument() {
        let cask = serde_json::json!({
            "token": "test",
            "version": "1.0.0",
            "url": "https://example.com/test.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        });

        let err = resolve_cask("test", &cask).unwrap_err();
        assert!(matches!(err, Error::InvalidArgument { .. }));
    }

    #[test]
    fn resolve_cask_accepts_app_only_casks() {
        let cask = serde_json::json!({
            "token": "ghostty",
            "version": "1.0.0",
            "url": "https://example.com/Ghostty.dmg",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [
                { "app": ["Ghostty.app"] },
                { "zap": [{ "trash": ["~/.config/ghostty/"] }] }
            ]
        });

        let resolved = resolve_cask("ghostty", &cask).unwrap();
        assert_eq!(resolved.apps.len(), 1);
        assert!(resolved.binaries.is_empty());
    }

    #[test]
    fn resolve_cask_no_supported_artifacts_lists_found_types() {
        let cask = serde_json::json!({
            "token": "zap-only",
            "version": "1.0.0",
            "url": "https://example.com/zap-only.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "artifacts": [
                { "zap": [{ "trash": ["~/Library/Application Support/Pkg"] }] }
            ]
        });

        let err = resolve_cask("zap-only", &cask).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no supported artifacts"), "got: {msg}");
        assert!(msg.contains("zap"), "got: {msg}");
    }
}
