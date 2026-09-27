//! Sub-10ms CLI runner: resolves local package binaries in `node_modules/.bin/`
//! and hands execution over directly via `execve`.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Find a binary by name in `node_modules/.bin` traversing up from `start_dir`,
/// or fallback to finding it in the system `PATH`.
pub fn find_binary(binary: &str, start_dir: &Path) -> Option<PathBuf> {
    let mut current = Some(start_dir);
    while let Some(dir) = current {
        let bin_path = dir.join("node_modules").join(".bin").join(binary);
        if bin_path.is_file() {
            return Some(bin_path);
        }
        if dir.join(".git").exists() {
            break;
        }
        current = dir.parent();
    }

    // Fallback: search system PATH
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(binary);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

/// Execute the binary with arguments, replacing the current process directly via `execve`.
pub fn run_exec(binary: &str, args: &[String], project_root: &Path) -> Result<i32> {
    let current_dir = std::env::current_dir().unwrap_or_else(|_| project_root.to_path_buf());
    let bin_path = find_binary(binary, &current_dir).ok_or_else(|| {
        Error::usage(format!(
            "binary '{binary}' not found in node_modules/.bin or PATH"
        ))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new(&bin_path);
        cmd.args(args);
        // Replace current process directly via execve syscall (< 1ms overhead)
        let err = cmd.exec();
        Err(Error::io(
            format!("cannot exec {}", bin_path.display()),
            err,
        ))
    }

    #[cfg(not(unix))]
    {
        let mut cmd = std::process::Command::new(&bin_path);
        cmd.args(args);
        let status = cmd
            .status()
            .map_err(|err| Error::io(format!("cannot execute {}", bin_path.display()), err))?;
        Ok(status.code().unwrap_or(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_binary_in_ancestor_node_modules_bin() {
        let temp = std::env::temp_dir().join(format!("kilnx-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let bin_dir = temp.join("node_modules/.bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let target_bin = bin_dir.join("my-tool");
        fs::write(&target_bin, "#!/bin/sh\necho hello\n").unwrap();

        let sub_dir = temp.join("packages/sub-package/src");
        fs::create_dir_all(&sub_dir).unwrap();

        let found = find_binary("my-tool", &sub_dir);
        assert_eq!(found, Some(target_bin));

        let _ = fs::remove_dir_all(&temp);
    }
}
