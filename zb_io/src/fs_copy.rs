use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use zb_core::Error;

pub(crate) fn copy_dir_with_fallback(src: &Path, dst: &Path) -> Result<(), Error> {
    #[cfg(target_os = "macos")]
    {
        if try_clonefile_dir(src, dst).is_ok() {
            return Ok(());
        }
    }

    #[cfg(target_os = "linux")]
    {
        if try_reflink_dir(src, dst).is_ok() {
            return Ok(());
        }
    }

    copy_dir_recursive(src, dst)
}

#[cfg(target_os = "macos")]
fn try_clonefile_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    if dst.exists() {
        fs::remove_dir_all(dst)?;
    }
    fs::create_dir_all(dst)?;

    let source_contents = src.join(".");
    let status = Command::new("/bin/cp")
        .arg("-cR")
        .arg(&source_contents)
        .arg(dst)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        let _ = fs::remove_dir_all(dst);
        Err(std::io::Error::other(
            "copy-on-write directory clone failed",
        ))
    }
}

#[cfg(target_os = "linux")]
fn try_reflink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    if dst.exists() {
        fs::remove_dir_all(dst)?;
    }
    fs::create_dir_all(dst)?;

    let source_contents = src.join(".");
    let status = Command::new("cp")
        .arg("--reflink=auto")
        .arg("-a")
        .arg(&source_contents)
        .arg(dst)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        let _ = fs::remove_dir_all(dst);
        Err(std::io::Error::other("reflink directory copy failed"))
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), Error> {
    let create_ctx = format!("failed to create directory {}", dst.display());
    fs::create_dir_all(dst).map_err(Error::store(create_ctx.as_str()))?;

    let read_ctx = format!("failed to read directory {}", src.display());
    for entry in fs::read_dir(src).map_err(Error::store(read_ctx.as_str()))? {
        let entry = entry.map_err(Error::store("failed to read directory entry"))?;

        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(Error::store("failed to get file type"))?;

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_symlink() {
            let target =
                fs::read_link(&src_path).map_err(Error::store("failed to read symlink"))?;

            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &dst_path)
                .map_err(Error::store("failed to create symlink"))?;

            #[cfg(not(unix))]
            fs::copy(&src_path, &dst_path)
                .map_err(Error::store("failed to copy symlink as file"))?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path).map_err(Error::store("failed to copy file"))?;

            #[cfg(unix)]
            {
                let metadata =
                    fs::metadata(&src_path).map_err(Error::store("failed to read metadata"))?;
                fs::set_permissions(&dst_path, metadata.permissions())
                    .map_err(Error::store("failed to set permissions"))?;
            }
        } else {
            return Err(Error::StoreCorruption {
                message: format!(
                    "unsupported file type in copy source: {}",
                    src_path.display()
                ),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn recursive_fallback_rejects_non_regular_files() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        let fifo = src.join("pipe");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();

        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o644) }, 0);

        let err = copy_dir_recursive(&src, &dst).unwrap_err();
        assert!(err.to_string().contains("unsupported file type"));
    }
}
