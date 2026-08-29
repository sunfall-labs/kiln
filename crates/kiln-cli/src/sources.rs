//! Finding the project's source files.
//!
//! The wire protocol has no file discovery in it: the daemon is fed sources by the client
//! (`SetSource`) and never touches the filesystem. That keeps the daemon deterministic and
//! makes `--one-shot` possible at all, but it means "the project" is whatever this walk
//! finds, so the walk has to be predictable: fixed extensions, a fixed skip list, and a
//! sorted result so interning order - and therefore every `FileId` - is reproducible.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const SOURCE_EXTENSIONS: [&str; 6] = ["js", "jsx", "mjs", "cjs", "ts", "tsx"];

/// Directories that are never project sources. Anything starting with `.` is skipped too.
const SKIP_DIRS: [&str; 5] = ["node_modules", "target", "dist", "build", "coverage"];

/// A ceiling that keeps a mistyped root (`kiln project` in `$HOME`) from turning into a
/// multi-minute walk. Hitting it is reported, never silently truncated.
const MAX_FILES: usize = 10_000;

pub struct Walk {
    pub files: Vec<PathBuf>,
    /// True when [`MAX_FILES`] cut the walk short.
    pub truncated: bool,
}

pub fn collect(root: &Path) -> Result<Walk> {
    let mut files = Vec::new();
    let mut truncated = false;
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|source| Error::io(format!("cannot read {}", dir.display()), source))?;
        for entry in entries {
            let entry = entry
                .map_err(|source| Error::io(format!("cannot read {}", dir.display()), source))?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            // `file_type` does not follow symlinks, so a link back up the tree cannot turn
            // the walk into a loop.
            let file_type = entry.file_type().map_err(|source| {
                Error::io(format!("cannot inspect {}", path.display()), source)
            })?;
            if file_type.is_dir() {
                if !SKIP_DIRS.contains(&name) {
                    stack.push(path);
                }
            } else if file_type.is_file() && is_source(&path) {
                if files.len() == MAX_FILES {
                    truncated = true;
                    stack.clear();
                    break;
                }
                files.push(path);
            }
        }
    }

    files.sort();
    Ok(Walk { files, truncated })
}

fn is_source(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SOURCE_EXTENSIONS.contains(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway tree under the OS temp directory, removed when the test ends.
    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Tree {
            let dir =
                std::env::temp_dir().join(format!("kiln-cli-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create test tree");
            Tree(dir)
        }

        fn file(&self, relative: &str) {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().expect("has a parent")).expect("create dirs");
            std::fs::write(&path, "export const x = 1;\n").expect("write file");
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn collects_sources_and_skips_the_usual_noise() {
        let tree = Tree::new("walk");
        tree.file("src/b.ts");
        tree.file("src/a.js");
        tree.file("src/nested/c.tsx");
        tree.file("readme.md");
        tree.file("node_modules/dep/index.js");
        tree.file("dist/bundle.js");
        tree.file(".hidden/secret.js");

        let walk = collect(&tree.0).expect("walk succeeds");
        let found: Vec<String> = walk
            .files
            .iter()
            .map(|p| {
                p.strip_prefix(&tree.0)
                    .expect("under root")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        assert_eq!(found, vec!["src/a.js", "src/b.ts", "src/nested/c.tsx"]);
        assert!(!walk.truncated);
    }

    #[test]
    fn the_result_is_sorted_so_file_ids_are_reproducible() {
        let tree = Tree::new("sorted");
        for name in ["z.js", "a.js", "m.ts"] {
            tree.file(name);
        }
        let first = collect(&tree.0).expect("walk succeeds").files;
        let second = collect(&tree.0).expect("walk succeeds").files;
        assert_eq!(first, second);
        assert!(first.windows(2).all(|pair| pair[0] < pair[1]), "{first:?}");
    }
}
