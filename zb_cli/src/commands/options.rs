use std::fmt::Write as _;
use std::sync::LazyLock;

use regex::Regex;
use std::path::Path;

static OPTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*option\s+"([^"]+)"\s*,\s*"([^"]+)""#).expect("OPTION_RE must compile")
});
static RECOMMENDED_DEP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*depends_on\s+["']([^"']+)["']\s*=>\s*:recommended"#)
        .expect("RECOMMENDED_DEP_RE must compile")
});

pub async fn execute(
    installer: &mut zb_io::Installer,
    repository: &Path,
    formulas: Vec<String>,
    compact: bool,
    command: Option<String>,
) -> Result<(), zb_core::Error> {
    if let Some(command) = command {
        return print_command_options(&command, compact);
    }
    if formulas.is_empty() {
        print_global_options(compact);
        return Ok(());
    }

    let multiple = formulas.len() > 1;
    for formula in formulas {
        let source = formula_source(installer, repository, &formula).await?;
        let mut options = formula_options(&source);
        if options.is_empty() {
            continue;
        }
        options.sort_by(|a, b| a.0.cmp(&b.0));
        if multiple {
            println!("{formula}");
        }
        print_options(&options, compact);
    }

    Ok(())
}

async fn formula_source(
    installer: &mut zb_io::Installer,
    repository: &Path,
    formula: &str,
) -> Result<String, zb_core::Error> {
    let path = crate::commands::edit::formula_path(
        &crate::commands::edit::repository_path(repository),
        formula,
    );
    if path.exists() {
        return std::fs::read_to_string(path)
            .map_err(zb_core::Error::file("failed to read formula source"));
    }

    installer.formula_source(formula).await
}

fn print_global_options(compact: bool) {
    let options = [
        ("--build-from-source".to_string(), String::new()),
        ("--force-bottle".to_string(), String::new()),
        ("--ignore-dependencies".to_string(), String::new()),
        ("--only-dependencies".to_string(), String::new()),
    ];
    print_options(&options, compact);
}

fn print_command_options(command: &str, compact: bool) -> Result<(), zb_core::Error> {
    if command == "install" {
        print_global_options(compact);
        return Ok(());
    }

    Err(zb_core::Error::InvalidArgument {
        message: format!("Unknown command: zb {command}"),
    })
}

fn format_options(options: &[(String, String)], compact: bool) -> String {
    let mut output = String::new();
    if compact {
        writeln!(
            output,
            "{}",
            options
                .iter()
                .map(|(flag, _)| flag.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        )
        .expect("writing to a String cannot fail");
        return output;
    }

    for (flag, description) in options {
        writeln!(output, "{flag}").expect("writing to a String cannot fail");
        if !description.is_empty() {
            writeln!(output, "\t{description}").expect("writing to a String cannot fail");
        }
    }
    output.push('\n');
    output
}

fn print_options(options: &[(String, String)], compact: bool) {
    print!("{}", format_options(options, compact));
}

fn formula_options(source: &str) -> Vec<(String, String)> {
    let mut options: Vec<(String, String)> = OPTION_RE
        .captures_iter(source)
        .filter_map(|cap| {
            Some((
                format!("--{}", cap.get(1)?.as_str()),
                cap.get(2)?.as_str().to_string(),
            ))
        })
        .collect();
    options.extend(RECOMMENDED_DEP_RE.captures_iter(source).filter_map(|cap| {
        Some((
            format!("--without-{}", cap.get(1)?.as_str()),
            format!("Build without {} support", cap.get(1)?.as_str()),
        ))
    }));
    options
}

#[cfg(test)]
mod tests {
    use super::{format_options, formula_options, print_command_options};

    #[test]
    fn formula_options_extracts_declared_and_recommended_options() {
        let source = r#"
            option "with-foo", "Build with package's foo"
            depends_on "bar" => :recommended
            depends_on "baz" => :build
        "#;

        assert_eq!(
            formula_options(source),
            vec![
                (
                    "--with-foo".to_string(),
                    "Build with package's foo".to_string()
                ),
                (
                    "--without-bar".to_string(),
                    "Build without bar support".to_string()
                )
            ]
        );
    }

    #[test]
    fn format_options_renders_multiline_output() {
        let options = vec![
            ("--with-foo".to_string(), "Build with foo".to_string()),
            (
                "--without-bar".to_string(),
                "Build without bar support".to_string(),
            ),
        ];

        assert_eq!(
            format_options(&options, false),
            "--with-foo\n\tBuild with foo\n--without-bar\n\tBuild without bar support\n\n"
        );
    }

    #[test]
    fn format_options_renders_compact_flags_only() {
        let options = vec![
            ("--with-foo".to_string(), "Build with foo".to_string()),
            (
                "--without-bar".to_string(),
                "Build without bar support".to_string(),
            ),
        ];

        assert_eq!(format_options(&options, true), "--with-foo --without-bar\n");
    }

    #[test]
    fn command_options_error_uses_zerobrew_branding() {
        let err = print_command_options("unknown", false).unwrap_err();

        assert!(err.to_string().contains("Unknown command: zb unknown"));
    }
}
