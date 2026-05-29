use crate::ui::StdUi;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub async fn execute(
    installer: &mut zb_io::Installer,
    repository: &Path,
    formulas: Vec<String>,
    cask: bool,
    print_path: bool,
    ui: &mut StdUi,
) -> Result<(), zb_core::Error> {
    let repository = repository_path(repository);
    let paths = edit_paths(&repository, &formulas, cask);
    if print_path {
        for path in paths {
            ui.println(path.display()).map_err(ui_error)?;
        }
        return Ok(());
    }

    let editor = editor()?;
    if !cask {
        write_missing_formula_files(installer, &paths, &formulas).await?;
    }
    let status = Command::new(editor)
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

fn repository_path(default: &Path) -> PathBuf {
    std::env::var_os("HOMEBREW_TEST_TMPDIR")
        .map(|path| PathBuf::from(path).join("prefix"))
        .unwrap_or_else(|| default.to_path_buf())
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

fn formula_path(repository: &Path, formula: &str) -> PathBuf {
    let token = formula.rsplit('/').next().unwrap_or(formula);
    repository
        .join("Library/Taps/homebrew/homebrew-core")
        .join("Formula")
        .join(format!("{token}.rb"))
}

fn cask_path(repository: &Path, cask: &str) -> PathBuf {
    let token = cask.rsplit('/').next().unwrap_or(cask);
    repository
        .join("Library/Taps/homebrew/homebrew-cask")
        .join("Casks")
        .join(format!("{token}.rb"))
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

fn ui_error(err: std::io::Error) -> zb_core::Error {
    zb_core::Error::FileError {
        message: format!("failed to write CLI output: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{edit_paths, repository_path};
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
    fn edit_paths_points_casks_at_homebrew_cask_files() {
        assert_eq!(
            edit_paths(Path::new("/repo"), &["iterm2".to_string()], true),
            vec![Path::new(
                "/repo/Library/Taps/homebrew/homebrew-cask/Casks/iterm2.rb"
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
    fn repository_path_prefers_homebrew_test_tmpdir() {
        unsafe {
            std::env::set_var("HOMEBREW_TEST_TMPDIR", "/tmp/homebrew-tests-demo");
        }

        assert_eq!(
            repository_path(Path::new("/repo")),
            Path::new("/tmp/homebrew-tests-demo/prefix")
        );

        unsafe {
            std::env::remove_var("HOMEBREW_TEST_TMPDIR");
        }
    }
}
