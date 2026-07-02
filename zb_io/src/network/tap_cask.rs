use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::LazyLock;
use zb_core::Error;

use crate::network::tap_formula::{TapFormulaRef, preprocess_tap_source};

static VERSION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*version\s+["']([^"']+)["']"#).expect("VERSION_RE must compile")
});
static URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)^\s*url\s+["']([^"']+)["']"#).expect("URL_RE must compile"));
static SHA_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*sha256\s+(?:["']([0-9a-f]{64}|no_check)["']|:(no_check))"#)
        .expect("SHA_RE must compile")
});
static SHA_KEYED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([A-Za-z0-9_]+):\s*["']([0-9a-f]{64}|no_check)["']"#)
        .expect("SHA_KEYED_RE must compile")
});
static NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*name\s+["']([^"']+)["']"#).expect("NAME_RE must compile")
});
static DESC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*desc\s+["']([^"']+)["']"#).expect("DESC_RE must compile")
});
static HOMEPAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*homepage\s+["']([^"']+)["']"#).expect("HOMEPAGE_RE must compile")
});
static CASK_START_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*cask\s+["'][^"']+["']\s+do\b"#).expect("CASK_START_RE must compile")
});
static END_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*end\b"#).expect("END_RE must compile"));
static DO_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bdo\b\s*(?:\|[^|]*\|\s*)?(?:#.*)?$"#).expect("DO_RE must compile")
});
static KEYWORD_START_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(if|unless|case|begin|def|class|module|for|while|until)\b"#)
        .expect("KEYWORD_START_RE must compile")
});
static HEREDOC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<<[-~]?['"]?([A-Za-z_][A-Za-z0-9_]*)['"]?"#).unwrap());
static ARTIFACT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(app|binary|font|pkg|suite|appimage)\s+(.+)$"#)
        .expect("ARTIFACT_RE must compile")
});
static GENERIC_ARTIFACT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*artifact\s+(.+)$"#).unwrap());
static STAGE_ONLY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*stage_only\s+true\b"#).unwrap());
static DEPENDS_ON_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*depends_on\s+(formula|cask):\s*(.+)$"#).unwrap());
static INSTALLER_MANUAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*installer\s+manual:\s*["']([^"']+)["']"#).unwrap());
static INSTALLER_SCRIPT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*installer\s+script:\s*\{(.+)\}\s*$"#).unwrap());
static QUOTED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"["']([^"']+)["']"#).unwrap());
static TARGET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"target:\s*["']([^"']+)["']"#).unwrap());
static EXECUTABLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"executable:\s*["']([^"']+)["']"#).unwrap());
static ARGS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"args:\s*\[([^\]]*)\]"#).unwrap());

pub(crate) fn parse_tap_cask_ruby(spec: &TapFormulaRef, source: &str) -> Result<Value, Error> {
    let source = preprocess_tap_source(source);
    let body = extract_cask_body(&source).unwrap_or(source.as_str());
    let mut artifact_entries: BTreeMap<&'static str, Vec<Value>> = BTreeMap::new();
    let mut depends_on_formulas = Vec::new();
    let mut depends_on_casks = Vec::new();
    let mut stage_only = false;
    let mut installers = Vec::new();
    let mut depth = 0usize;
    let mut heredoc_end: Option<String> = None;

    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(ref terminator) = heredoc_end {
            if trimmed == terminator {
                heredoc_end = None;
            }
            continue;
        }

        if depth == 0 {
            if let Some(cap) = ARTIFACT_RE.captures(trimmed) {
                let kind = match cap.get(1).map(|m| m.as_str()).unwrap_or_default() {
                    "app" => "app",
                    "binary" => "binary",
                    "font" => "font",
                    "pkg" => "pkg",
                    "suite" => "suite",
                    "appimage" => "appimage",
                    _ => continue,
                };
                let args = cap.get(2).map(|m| m.as_str()).unwrap_or_default();
                if let Some(entry) = artifact_entry(args) {
                    artifact_entries.entry(kind).or_default().push(entry);
                }
            } else if let Some(cap) = GENERIC_ARTIFACT_RE.captures(trimmed) {
                let args = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
                if let Some(entry) = artifact_entry(args) {
                    artifact_entries.entry("artifact").or_default().push(entry);
                }
            } else if STAGE_ONLY_RE.is_match(trimmed) {
                stage_only = true;
            } else if let Some(cap) = DEPENDS_ON_RE.captures(trimmed) {
                let kind = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
                let values = quoted_values(cap.get(2).map(|m| m.as_str()).unwrap_or_default());
                match kind {
                    "formula" => depends_on_formulas.extend(values),
                    "cask" => depends_on_casks.extend(values),
                    _ => {}
                }
            } else if let Some(cap) = INSTALLER_MANUAL_RE.captures(trimmed) {
                if let Some(path) = cap.get(1).map(|m| m.as_str()) {
                    installers.push(json!({ "manual": path }));
                }
            } else if let Some(cap) = INSTALLER_SCRIPT_RE.captures(trimmed)
                && let Some(script) = installer_script(cap.get(1).map(|m| m.as_str()).unwrap_or(""))
            {
                installers.push(script);
            }
        }

        if let Some(cap) = HEREDOC_RE.captures(trimmed)
            && let Some(terminator) = cap.get(1)
        {
            heredoc_end = Some(terminator.as_str().to_string());
        }
        update_depth(&mut depth, trimmed);
    }

    let mut artifacts = artifact_entries
        .into_iter()
        .map(|(kind, entries)| json!({ kind: entries }))
        .collect::<Vec<_>>();
    if !installers.is_empty() {
        artifacts.push(json!({ "installer": installers }));
    }
    if stage_only {
        artifacts.push(json!({ "stage_only": [true] }));
    }

    let mut cask = Map::new();
    cask.insert("token".to_string(), json!(spec.formula));
    cask.insert(
        "version".to_string(),
        json!(required_capture(&VERSION_RE, &source, "version")?),
    );
    cask.insert(
        "url".to_string(),
        json!(required_capture(&URL_RE, &source, "url")?),
    );
    cask.insert("sha256".to_string(), json!(required_sha(&source)?));
    cask.insert("artifacts".to_string(), Value::Array(artifacts));

    if let Some(name) = optional_capture(&NAME_RE, &source) {
        cask.insert("name".to_string(), json!([name]));
    }
    if let Some(desc) = optional_capture(&DESC_RE, &source) {
        cask.insert("desc".to_string(), json!(desc));
    }
    if let Some(homepage) = optional_capture(&HOMEPAGE_RE, &source) {
        cask.insert("homepage".to_string(), json!(homepage));
    }
    if !depends_on_formulas.is_empty() || !depends_on_casks.is_empty() {
        let mut depends_on = Map::new();
        if !depends_on_formulas.is_empty() {
            depends_on.insert("formula".to_string(), json!(depends_on_formulas));
        }
        if !depends_on_casks.is_empty() {
            depends_on.insert("cask".to_string(), json!(depends_on_casks));
        }
        cask.insert("depends_on".to_string(), Value::Object(depends_on));
    }

    Ok(Value::Object(cask))
}

fn artifact_entry(args: &str) -> Option<Value> {
    let source = first_quoted(args)?;
    let target = TARGET_RE
        .captures(args)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string());

    Some(match target {
        Some(target) => json!([source, { "target": target }]),
        None => json!([source]),
    })
}

fn installer_script(args: &str) -> Option<Value> {
    let executable = EXECUTABLE_RE
        .captures(args)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())?;
    let script_args = ARGS_RE
        .captures(args)
        .and_then(|cap| cap.get(1))
        .map(|m| quoted_values(m.as_str()))
        .unwrap_or_default();

    Some(json!({
        "script": {
            "executable": executable,
            "args": script_args,
        }
    }))
}

fn first_quoted(input: &str) -> Option<String> {
    QUOTED_RE
        .captures(input)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
}

fn quoted_values(input: &str) -> Vec<String> {
    QUOTED_RE
        .captures_iter(input)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

fn required_capture(regex: &Regex, source: &str, field: &str) -> Result<String, Error> {
    optional_capture(regex, source).ok_or_else(|| Error::InvalidArgument {
        message: format!("failed to parse tap cask Ruby: missing {field}"),
    })
}

fn optional_capture(regex: &Regex, source: &str) -> Option<String> {
    regex
        .captures(source)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
}

fn required_sha(source: &str) -> Result<String, Error> {
    for line in source.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("sha256 ") {
            continue;
        }

        if let Some(cap) = SHA_RE.captures(trimmed)
            && let Some(value) = cap.get(1).or_else(|| cap.get(2))
        {
            return Ok(value.as_str().to_string());
        }

        let keyed = SHA_KEYED_RE
            .captures_iter(trimmed)
            .filter_map(|cap| {
                let key = cap.get(1)?.as_str();
                let value = cap.get(2)?.as_str();
                Some((key, value))
            })
            .collect::<Vec<_>>();

        for preferred in preferred_sha_keys() {
            if let Some((_, value)) = keyed.iter().find(|(key, _)| key == preferred) {
                return Ok((*value).to_string());
            }
        }

        if let Some((_, value)) = keyed.first() {
            return Ok((*value).to_string());
        }
    }

    Err(Error::InvalidArgument {
        message: "failed to parse tap cask Ruby: missing sha256".to_string(),
    })
}

fn preferred_sha_keys() -> &'static [&'static str] {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        &["x86_64_linux", "x86_64", "intel"]
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        &["arm64_linux", "arm64", "arm"]
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        &["arm64", "arm"]
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        &["intel", "x86_64"]
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64")
    )))]
    {
        &[]
    }
}

fn extract_cask_body(source: &str) -> Option<&str> {
    let mut offset = 0usize;
    let mut body_start: Option<usize> = None;
    let mut depth = 0usize;
    let mut heredoc_end: Option<String> = None;

    for line in source.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let trimmed = line.trim();

        if body_start.is_none() {
            if CASK_START_RE.is_match(trimmed) {
                body_start = Some(offset);
                depth = 1;
            }
            continue;
        }

        if let Some(ref terminator) = heredoc_end {
            if trimmed == terminator {
                heredoc_end = None;
            }
            continue;
        }

        if let Some(cap) = HEREDOC_RE.captures(trimmed)
            && let Some(terminator) = cap.get(1)
        {
            heredoc_end = Some(terminator.as_str().to_string());
        }

        let depth_before = depth;
        update_depth(&mut depth, trimmed);
        if depth_before > 0 && depth == 0 {
            return body_start.map(|start| &source[start..line_start]);
        }
    }

    None
}

fn update_depth(depth: &mut usize, trimmed: &str) {
    if END_RE.is_match(trimmed) {
        *depth = depth.saturating_sub(1);
        return;
    }

    *depth += DO_RE.find_iter(trimmed).count();
    if KEYWORD_START_RE.is_match(trimmed) {
        *depth += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::tap_formula::TapFormulaRef;

    fn spec() -> TapFormulaRef {
        TapFormulaRef {
            owner: "kamillobinski".to_string(),
            repo: "thock".to_string(),
            formula: "thock".to_string(),
        }
    }

    #[test]
    fn parses_tap_cask_ruby() {
        let rb = r##"
cask "thock" do
  version "1.23.0"
  sha256 "2f7bd6093e8e26aec43f2c48f0c2efefbc0fd22f81bc8788d745304047d4060c"

  url "https://github.com/kamillobinski/thock/releases/download/#{version}/Thock-#{version}.zip"
  name "Thock"
  desc "Thock your mac keyboard"
  homepage "https://github.com/kamillobinski/thock"

  app "Thock.app"
  binary "thock-cli"

  caveats <<~EOS
    CLI: thock-cli
  EOS

  postflight do
    system_command "xattr", args: ["-cr", "#{appdir}/Thock.app"], sudo: false
  end
end
"##;

        let cask = parse_tap_cask_ruby(&spec(), rb).unwrap();

        assert_eq!(cask["token"], "thock");
        assert_eq!(cask["version"], "1.23.0");
        assert_eq!(
            cask["url"],
            "https://github.com/kamillobinski/thock/releases/download/1.23.0/Thock-1.23.0.zip"
        );
        assert_eq!(cask["artifacts"][0]["app"][0][0], "Thock.app");
        assert_eq!(cask["artifacts"][1]["binary"][0][0], "thock-cli");
    }

    #[test]
    fn parses_targets_dependencies_installers_and_stage_only() {
        let rb = r#"
cask "demo" do
  version "2.0.0"
  sha256 :no_check
  url "https://example.com/demo.zip"

  depends_on formula: ["openssl@3", "python@3.13"]
  depends_on cask: "xquartz"
  binary "bin/demo", target: "demo"
  artifact "share/demo.conf", target: "etc/demo.conf"
  installer manual: "Install Demo.app"
  installer script: { executable: "install.sh", args: ["--prefix", "$HOMEBREW_PREFIX"] }
  stage_only true
end
"#;

        let cask = parse_tap_cask_ruby(&spec(), rb).unwrap();

        assert_eq!(cask["sha256"], "no_check");
        assert_eq!(cask["depends_on"]["formula"][0], "openssl@3");
        assert_eq!(cask["depends_on"]["cask"][0], "xquartz");
        assert_eq!(
            cask["artifacts"][0]["artifact"][0][1]["target"],
            "etc/demo.conf"
        );
        assert_eq!(cask["artifacts"][1]["binary"][0][1]["target"], "demo");
        assert_eq!(
            cask["artifacts"][2]["installer"][0]["manual"],
            "Install Demo.app"
        );
        assert_eq!(
            cask["artifacts"][2]["installer"][1]["script"]["args"][1],
            "$HOMEBREW_PREFIX"
        );
        assert_eq!(cask["artifacts"][3]["stage_only"][0], true);
    }

    #[test]
    fn parses_platform_qualified_sha256() {
        let arm_sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let intel_sha = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let rb = format!(
            r#"
cask "demo" do
  version "2.0.0"
  sha256 arm: "{arm_sha}", intel: "{intel_sha}"
  url "https://example.com/demo.zip"

  binary "demo"
end
"#
        );

        let cask = parse_tap_cask_ruby(&spec(), &rb).unwrap();

        #[cfg(any(target_arch = "aarch64", target_arch = "arm"))]
        assert_eq!(cask["sha256"], arm_sha);
        #[cfg(not(any(target_arch = "aarch64", target_arch = "arm")))]
        assert_eq!(cask["sha256"], intel_sha);
    }

    #[test]
    fn extract_cask_body_ignores_end_inside_heredoc() {
        let rb = r#"
cask "demo" do
  version "2.0.0"
  sha256 :no_check
  url "https://example.com/demo.zip"

  caveats <<~EOS
    end
  EOS

  binary "demo"
end
"#;

        let cask = parse_tap_cask_ruby(&spec(), rb).unwrap();

        assert_eq!(cask["artifacts"][0]["binary"][0][0], "demo");
    }
}
