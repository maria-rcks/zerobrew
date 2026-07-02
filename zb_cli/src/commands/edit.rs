use crate::ui::Ui;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub async fn execute(
    installer: &mut zb_io::Installer,
    repository: &Path,
    formulas: Vec<String>,
    cask: bool,
    print_path: bool,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    let repository = repository_path(repository);
    let paths = edit_paths(&repository, &formulas, cask);
    if print_path {
        for path in paths {
            ui.data(path.display());
        }
        return Ok(());
    }

    let editor = editor()?;
    if !cask {
        write_missing_formula_files(installer, &paths, &formulas).await?;
    }
    let mut editor_parts = editor.split_whitespace();
    let editor_program = editor_parts
        .next()
        .ok_or_else(|| zb_core::Error::ExecutionError {
            message: "no editor set; set $HOMEBREW_EDITOR or $EDITOR".to_string(),
        })?;
    let status = Command::new(editor_program)
        .args(editor_parts)
        .args(&paths)
        .status()
        .map_err(zb_core::Error::exec("failed to run editor"))?;
    if status.success() {
        Ok(())
    } else {
        Err(zb_core::Error::ExecutionError {
            message: "editor exited unsuccessfully".to_string(),
        })
    }
}

async fn write_missing_formula_files(
    installer: &mut zb_io::Installer,
    paths: &[PathBuf],
    formulas: &[String],
) -> Result<(), zb_core::Error> {
    for (path, formula) in paths.iter().zip(formulas) {
        if path.exists() {
            continue;
        }

        let source = installer.formula_source(formula).await?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(zb_core::Error::file(
                "failed to create formula source directory",
            ))?;
        }
        fs::write(path, source).map_err(zb_core::Error::file("failed to write formula source"))?;
    }

    Ok(())
}

pub(crate) fn repository_path(default: &Path) -> PathBuf {
    if std::env::var_os("HOMEBREW_INTEGRATION_TEST").is_some()
        && let Some(path) = std::env::var_os("HOMEBREW_TEST_TMPDIR")
    {
        return PathBuf::from(path).join("prefix");
    }

    default.to_path_buf()
}

fn edit_paths(repository: &Path, formulas: &[String], cask: bool) -> Vec<PathBuf> {
    if formulas.is_empty() {
        return vec![repository.to_path_buf()];
    }

    formulas
        .iter()
        .map(|formula| {
            if cask {
                cask_path(repository, formula)
            } else {
                formula_path(repository, formula)
            }
        })
        .collect()
}

pub(crate) fn formula_path(repository: &Path, formula: &str) -> PathBuf {
    let (tap_path, token) = formula_tap_path(formula);
    repository
        .join("Library/Taps")
        .join(tap_path)
        .join("Formula")
        .join(format!("{token}.rb"))
}

fn formula_tap_path(formula: &str) -> (PathBuf, &str) {
    let mut parts = formula.split('/');
    let (Some(owner), Some(repo), Some(token), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return (PathBuf::from("homebrew/homebrew-core"), formula);
    };

    (PathBuf::from(owner).join(format!("homebrew-{repo}")), token)
}

fn cask_path(repository: &Path, cask: &str) -> PathBuf {
    let (tap_path, token) = cask_tap_path(cask);
    repository
        .join("Library/Taps")
        .join(tap_path)
        .join("Casks")
        .join(format!("{token}.rb"))
}

fn cask_tap_path(cask: &str) -> (PathBuf, &str) {
    let mut parts = cask.split('/');
    let (Some(owner), Some(repo), Some(token), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return (PathBuf::from("homebrew/homebrew-cask"), cask);
    };

    (PathBuf::from(owner).join(format!("homebrew-{repo}")), token)
}

fn editor() -> Result<String, zb_core::Error> {
    for variable in ["HOMEBREW_EDITOR", "EDITOR"] {
        if let Ok(value) = std::env::var(variable)
            && !value.trim().is_empty()
        {
            return Ok(value);
        }
    }

    Err(zb_core::Error::ExecutionError {
        message: "no editor set; set $HOMEBREW_EDITOR or $EDITOR".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{cask_tap_path, edit_paths, formula_tap_path};
    use std::path::Path;

    #[test]
    fn edit_paths_points_formulae_at_core_tap_files() {
        assert_eq!(
            edit_paths(Path::new("/repo"), &["testball".to_string()], false),
            vec![Path::new(
                "/repo/Library/Taps/homebrew/homebrew-core/Formula/testball.rb"
            )]
        );
    }

    #[test]
    fn edit_paths_points_tapped_formulae_at_tap_files() {
        assert_eq!(
            edit_paths(
                Path::new("/repo"),
                &["hashicorp/tap/terraform".to_string()],
                false,
            ),
            vec![Path::new(
                "/repo/Library/Taps/hashicorp/homebrew-tap/Formula/terraform.rb"
            )]
        );
    }

    #[test]
    fn edit_paths_points_casks_at_homebrew_cask_files() {
        assert_eq!(
            edit_paths(Path::new("/repo"), &["iterm2".to_string()], true),
            vec![Path::new(
                "/repo/Library/Taps/homebrew/homebrew-cask/Casks/iterm2.rb"
            )]
        );
    }

    #[test]
    fn edit_paths_points_tapped_casks_at_tap_files() {
        assert_eq!(
            edit_paths(Path::new("/repo"), &["owner/tap/app".to_string()], true),
            vec![Path::new(
                "/repo/Library/Taps/owner/homebrew-tap/Casks/app.rb"
            )]
        );
    }

    #[test]
    fn edit_paths_opens_repository_without_arguments() {
        assert_eq!(
            edit_paths(Path::new("/repo"), &[], false),
            vec![Path::new("/repo")]
        );
    }

    #[test]
    fn formula_tap_path_ignores_invalid_tap_qualified_names() {
        assert_eq!(
            formula_tap_path("owner/repo/token/extra"),
            (
                Path::new("homebrew/homebrew-core").to_path_buf(),
                "owner/repo/token/extra"
            )
        );
    }

    #[test]
    fn cask_tap_path_ignores_invalid_tap_qualified_names() {
        assert_eq!(
            cask_tap_path("owner/repo/token/extra"),
            (
                Path::new("homebrew/homebrew-cask").to_path_buf(),
                "owner/repo/token/extra"
            )
        );
    }
}
