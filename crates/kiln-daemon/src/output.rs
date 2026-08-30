//! Default publication paths for native artifacts.
//!
//! The path is part of the user-facing build contract: two entry modules must not silently
//! publish over one another merely because they share a filename.  Keep the policy here so the
//! daemon, its in-process backend, and the CLI all derive exactly the same location.

use std::path::{Component, Path, PathBuf};

use kiln_digest::Digest;

/// Default executable path for `source` under `project_root`.
///
/// A project-relative source keeps its complete relative filename below `.kiln/bin`, so both
/// `src/main.ts` versus `tests/main.ts` and `src/main.ts` versus `src/main.js` remain distinct.
/// Sources outside the project are placed below `_external/<full path digest>` so two same-named
/// external entries cannot collide and no source component can escape the publication directory.
pub fn default_native_output(project_root: &Path, source: &Path) -> PathBuf {
    with_platform_extension(
        project_root
            .join(".kiln/bin")
            .join(output_identity(project_root, source)),
        false,
    )
}

/// Default shared-library path for `source` under `project_root`.
pub fn default_native_shared_output(project_root: &Path, source: &Path) -> PathBuf {
    with_platform_extension(
        project_root
            .join(".kiln/lib")
            .join(output_identity(project_root, source)),
        true,
    )
}

fn output_identity(project_root: &Path, source: &Path) -> PathBuf {
    let relative = if source.is_relative() {
        Some(source.to_path_buf())
    } else {
        source
            .strip_prefix(project_root)
            .ok()
            .map(Path::to_path_buf)
            .or_else(|| {
                let canonical_root = std::fs::canonicalize(project_root).ok()?;
                let canonical_source = std::fs::canonicalize(source).ok()?;
                canonical_source
                    .strip_prefix(canonical_root)
                    .ok()
                    .map(Path::to_path_buf)
            })
    };

    if let Some(identity) = relative.filter(|path| safe_relative(path))
        && identity.file_name().is_some_and(|name| !name.is_empty())
    {
        return identity;
    }

    let file_name = source
        .file_name()
        .filter(|name| !name.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("module"));
    let identity = if source.is_absolute() {
        source.to_path_buf()
    } else {
        project_root.join(source)
    };
    let identity = std::fs::canonicalize(&identity).unwrap_or(identity);
    let digest = Digest::hash(identity.as_os_str().as_encoded_bytes()).to_hex();
    Path::new("_external").join(digest).join(file_name)
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn with_platform_extension(output: PathBuf, shared: bool) -> PathBuf {
    let extension = if shared {
        Some(if cfg!(target_os = "macos") {
            "dylib"
        } else if cfg!(windows) {
            "dll"
        } else {
            "so"
        })
    } else if cfg!(windows) {
        Some("exe")
    } else {
        None
    };
    match extension {
        Some(extension) => {
            let mut name = output.into_os_string();
            name.push(".");
            name.push(extension);
            PathBuf::from(name)
        }
        None => output,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn executable_name(name: &str) -> String {
        if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        }
    }

    fn shared_name(name: &str) -> String {
        let extension = if cfg!(target_os = "macos") {
            "dylib"
        } else if cfg!(windows) {
            "dll"
        } else {
            "so"
        };
        format!("{name}.{extension}")
    }

    #[test]
    fn project_relative_directories_distinguish_same_named_entries() {
        let root = Path::new("/tmp/project");

        assert_eq!(
            default_native_output(root, Path::new("src/main.ts")),
            root.join(".kiln/bin/src").join(executable_name("main.ts"))
        );
        assert_eq!(
            default_native_output(root, Path::new("tests/main.ts")),
            root.join(".kiln/bin/tests")
                .join(executable_name("main.ts"))
        );
        assert_ne!(
            default_native_output(root, Path::new("src/main.ts")),
            default_native_output(root, Path::new("tests/main.ts"))
        );
    }

    #[test]
    fn source_extensions_distinguish_same_directory_entries() {
        let root = Path::new("/tmp/project");

        assert_eq!(
            default_native_output(root, Path::new("main.ts")),
            root.join(".kiln/bin").join(executable_name("main.ts"))
        );
        assert_ne!(
            default_native_output(root, Path::new("src/main.ts")),
            default_native_output(root, Path::new("src/main.js"))
        );
        assert_ne!(
            default_native_shared_output(root, Path::new("src/main.ts")),
            default_native_shared_output(root, Path::new("src/main.js"))
        );
    }

    #[test]
    fn shared_libraries_preserve_the_relative_directory() {
        let root = Path::new("/tmp/project");

        assert_eq!(
            default_native_shared_output(root, Path::new("src/main.ts")),
            root.join(".kiln/lib/src").join(shared_name("main.ts"))
        );
    }

    #[test]
    fn outside_sources_are_contained_and_same_named_entries_do_not_collide() {
        let root = Path::new("/tmp/project");
        let first = default_native_output(root, Path::new("../one/main.ts"));
        let second = default_native_output(root, Path::new("../two/main.ts"));

        assert!(first.starts_with(root.join(".kiln/bin/_external")));
        assert!(second.starts_with(root.join(".kiln/bin/_external")));
        assert_eq!(first.file_name(), Some(executable_name("main.ts").as_ref()));
        assert_eq!(
            second.file_name(),
            Some(executable_name("main.ts").as_ref())
        );
        assert_ne!(first, second);
        assert!(!first.components().any(|part| part == Component::ParentDir));
        assert!(!second.components().any(|part| part == Component::ParentDir));
    }
}
