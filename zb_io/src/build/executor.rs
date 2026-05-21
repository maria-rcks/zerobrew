use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use zb_core::{BuildPlan, Error};

use super::environment::build_env;
use super::source::download_and_extract_source;

const SHIM_RUBY: &str = include_str!("shim.rb");

pub struct BuildExecutor {
    prefix: PathBuf,
    work_root: PathBuf,
}

impl BuildExecutor {
    pub fn new(prefix: PathBuf) -> Self {
        let work_root = prefix.join("tmp").join("build");
        Self { prefix, work_root }
    }

    pub async fn execute(
        &self,
        plan: &BuildPlan,
        formula_rb_path: &Path,
        installed_deps: &HashMap<String, DepInfo>,
    ) -> Result<(), Error> {
        let work_dir = self.work_root.join(&plan.formula_name);
        self.prepare_work_dir(&work_dir).await?;

        let source_root = download_and_extract_source(
            &plan.source_url,
            plan.source_checksum.as_deref(),
            &work_dir,
        )
        .await?;

        let shim_path = work_dir.join("zerobrew_shim.rb");
        fs::write(&shim_path, SHIM_RUBY)
            .await
            .map_err(Error::file("failed to write ruby shim"))?;

        fs::create_dir_all(&plan.cellar_path)
            .await
            .map_err(Error::file("failed to create cellar directory"))?;

        let mut env = build_env(plan, &self.prefix);
        env.insert(
            "ZEROBREW_FORMULA_FILE".into(),
            formula_rb_path.display().to_string(),
        );

        let deps_json = serde_json::to_string(installed_deps).unwrap_or_else(|_| "{}".into());
        env.insert("ZEROBREW_INSTALLED_DEPS".into(), deps_json);

        let ruby = find_ruby().await?;
        run_build(&ruby, &shim_path, &source_root, &env).await?;

        self.cleanup_work_dir(&work_dir).await;
        Ok(())
    }

    async fn prepare_work_dir(&self, work_dir: &Path) -> Result<(), Error> {
        if work_dir.exists() {
            let _ = fs::remove_dir_all(work_dir).await;
        }
        fs::create_dir_all(work_dir)
            .await
            .map_err(Error::file("failed to create work directory"))
    }

    async fn cleanup_work_dir(&self, work_dir: &Path) {
        let _ = fs::remove_dir_all(work_dir).await;
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DepInfo {
    pub cellar_path: String,
}

async fn find_ruby() -> Result<PathBuf, Error> {
    for candidate in ["ruby", "/usr/bin/ruby"] {
        let result = Command::new(candidate).arg("--version").output().await;

        if let Ok(output) = result
            && output.status.success()
        {
            return Ok(PathBuf::from(candidate));
        }
    }

    Err(Error::ExecutionError {
        message: "ruby not found — required for building from source".into(),
    })
}

async fn run_build(
    ruby: &Path,
    shim_path: &Path,
    source_root: &Path,
    env: &HashMap<String, String>,
) -> Result<(), Error> {
    let mut child = Command::new(ruby)
        .arg(shim_path)
        .current_dir(source_root)
        .envs(env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(Error::exec("failed to execute ruby shim"))?;

    let stdout = child.stdout.take().ok_or_else(|| Error::ExecutionError {
        message: "failed to capture ruby shim stdout".to_string(),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| Error::ExecutionError {
        message: "failed to capture ruby shim stderr".to_string(),
    })?;

    let stdout_task = tokio::spawn(stream_output_and_capture_tail(stdout, false));
    let stderr_task = tokio::spawn(stream_output_and_capture_tail(stderr, true));

    let status = child
        .wait()
        .await
        .map_err(Error::exec("failed waiting for ruby shim"))?;

    let stdout_tail = stdout_task
        .await
        .map_err(Error::exec("failed to join stdout task"))?
        .map_err(Error::exec("failed reading stdout"))?;
    let stderr_tail = stderr_task
        .await
        .map_err(Error::exec("failed to join stderr task"))?
        .map_err(Error::exec("failed reading stderr"))?;

    if !status.success() {
        let mut msg = format!("source build failed (exit code: {:?})", status.code());
        let tail = if !stderr_tail.is_empty() {
            stderr_tail
        } else {
            stdout_tail
        };
        if !tail.is_empty() {
            msg.push('\n');
            msg.push_str(&tail.join("\n"));
        }
        return Err(Error::ExecutionError { message: msg });
    }

    Ok(())
}

async fn stream_output_and_capture_tail<R>(
    reader: R,
    stderr: bool,
) -> Result<Vec<String>, std::io::Error>
where
    R: AsyncRead + Unpin,
{
    const TAIL_LINES: usize = 40;
    let mut tail = VecDeque::with_capacity(TAIL_LINES);
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        if stderr {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }

        if tail.len() == TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }

    Ok(tail.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn run_build_supports_mv_in_formula_install() {
        let Some(ruby) = find_ruby().await.ok() else {
            return;
        };

        let tmp = tempfile::tempdir().unwrap();
        let source_root = tmp.path().join("source");
        std::fs::create_dir_all(source_root.join("themes")).unwrap();
        std::fs::write(source_root.join("themes/default.omp.json"), "{}").unwrap();

        let shim_path = tmp.path().join("shim.rb");
        std::fs::write(&shim_path, SHIM_RUBY).unwrap();

        let formula_path = tmp.path().join("foo.rb");
        std::fs::write(
            &formula_path,
            r#"
class Foo < Formula
  def install
    mv "themes", prefix
  end
end
"#,
        )
        .unwrap();

        let prefix = tmp.path().join("prefix");
        let cellar = prefix.join("Cellar");
        std::fs::create_dir_all(&cellar).unwrap();

        let env = shim_env(&prefix, &cellar, &formula_path, "foo", "1.0.0");

        run_build(&ruby, &shim_path, &source_root, &env)
            .await
            .unwrap();

        assert!(
            prefix
                .join("Cellar")
                .join("foo")
                .join("1.0.0")
                .join("themes")
                .join("default.omp.json")
                .exists()
        );
    }

    #[tokio::test]
    async fn run_build_includes_stderr_tail_in_error() {
        let Some(ruby) = find_ruby().await.ok() else {
            return;
        };

        let tmp = tempfile::tempdir().unwrap();
        let source_root = tmp.path().join("source");
        std::fs::create_dir_all(&source_root).unwrap();

        let shim_path = tmp.path().join("shim.rb");
        std::fs::write(&shim_path, SHIM_RUBY).unwrap();

        let formula_path = tmp.path().join("foo.rb");
        std::fs::write(
            &formula_path,
            r#"
class Foo < Formula
  def install
    system "sh", "-c", "echo boom-from-stderr 1>&2; exit 7"
  end
end
"#,
        )
        .unwrap();

        let prefix = tmp.path().join("prefix");
        let cellar = prefix.join("Cellar");
        std::fs::create_dir_all(&cellar).unwrap();

        let env = shim_env(&prefix, &cellar, &formula_path, "foo", "1.0.0");

        let err = run_build(&ruby, &shim_path, &source_root, &env)
            .await
            .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("source build failed"));
        assert!(message.contains("boom-from-stderr"));
    }

    #[tokio::test]
    async fn run_build_supports_utils_safe_popen_read_for_completions() {
        let Some(ruby) = find_ruby().await.ok() else {
            return;
        };

        let tmp = tempfile::tempdir().unwrap();
        let source_root = tmp.path().join("source");
        std::fs::create_dir_all(source_root.join("bin")).unwrap();

        let depot_bin = source_root.join("bin/depot");
        std::fs::write(
            &depot_bin,
            r#"#!/bin/sh
if [ "$1" = "completion" ]; then
  printf "%s completion\n" "$2"
  exit 0
fi
exit 1
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&depot_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&depot_bin, permissions).unwrap();

        let shim_path = tmp.path().join("shim.rb");
        std::fs::write(&shim_path, SHIM_RUBY).unwrap();

        let formula_path = tmp.path().join("depot.rb");
        std::fs::write(
            &formula_path,
            r##"
class Depot < Formula
  def install
    bin.install "bin/depot"

    bash_comp = Utils.safe_popen_read("#{bin}/depot", "completion", "bash")
    fish_comp = Utils.safe_popen_read("#{bin}/depot", "completion", "fish")
    zsh_comp = Utils.safe_popen_read("#{bin}/depot", "completion", "zsh")

    (bash_completion/"depot").write bash_comp
    (fish_completion/"depot.fish").write fish_comp
    (zsh_completion/"_depot").write zsh_comp
  end
end
"##,
        )
        .unwrap();

        let prefix = tmp.path().join("prefix");
        let cellar = prefix.join("Cellar");
        std::fs::create_dir_all(&cellar).unwrap();
        let env = shim_env(&prefix, &cellar, &formula_path, "depot", "2.101.63");

        run_build(&ruby, &shim_path, &source_root, &env)
            .await
            .unwrap();

        let keg = prefix.join("Cellar").join("depot").join("2.101.63");
        assert_eq!(
            std::fs::read_to_string(keg.join("etc/bash_completion.d/depot")).unwrap(),
            "bash completion\n"
        );
        assert_eq!(
            std::fs::read_to_string(keg.join("share/fish/vendor_completions.d/depot.fish"))
                .unwrap(),
            "fish completion\n"
        );
        assert_eq!(
            std::fs::read_to_string(keg.join("share/zsh/site-functions/_depot")).unwrap(),
            "zsh completion\n"
        );
    }

    fn shim_env(
        prefix: &Path,
        cellar: &Path,
        formula_path: &Path,
        formula_name: &str,
        formula_version: &str,
    ) -> HashMap<String, String> {
        HashMap::from([
            ("ZEROBREW_PREFIX".to_string(), prefix.display().to_string()),
            ("ZEROBREW_CELLAR".to_string(), cellar.display().to_string()),
            (
                "ZEROBREW_FORMULA_NAME".to_string(),
                formula_name.to_string(),
            ),
            (
                "ZEROBREW_FORMULA_VERSION".to_string(),
                formula_version.to_string(),
            ),
            (
                "ZEROBREW_FORMULA_FILE".to_string(),
                formula_path.display().to_string(),
            ),
            ("ZEROBREW_INSTALLED_DEPS".to_string(), "{}".to_string()),
        ])
    }
}
