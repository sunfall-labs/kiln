//! Finding the project's source files.
//!
//! The wire protocol has no file discovery in it: the daemon is fed sources by the client
//! (`SetSource`) and never touches the filesystem. That keeps the daemon deterministic and
//! makes `--one-shot` possible at all, but it means "the project" is whatever this walk
//! finds, so the walk has to be predictable: fixed extensions, a fixed skip list, and a
//! sorted depth-first traversal so even a bounded result - and therefore every `FileId` - is
//! reproducible.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const SOURCE_EXTENSIONS: [&str; 8] = ["js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts"];

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
    collect_with_limit(root, MAX_FILES)
}

fn collect_with_limit(root: &Path, max_files: usize) -> Result<Walk> {
    enum Work {
        Directory(PathBuf),
        Source(PathBuf),
    }

    let mut files = Vec::new();
    let mut truncated = false;
    let mut stack = vec![Work::Directory(root.to_path_buf())];

    while let Some(work) = stack.pop() {
        match work {
            Work::Source(path) => {
                if files.len() == max_files {
                    truncated = true;
                    break;
                }
                files.push(path);
            }
            Work::Directory(dir) => {
                let entries = std::fs::read_dir(&dir)
                    .map_err(|source| Error::io(format!("cannot read {}", dir.display()), source))?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|source| {
                        Error::io(format!("cannot read {}", dir.display()), source)
                    })?;
                let mut children = Vec::with_capacity(entries.len());
                for entry in entries {
                    let path = entry.path();
                    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    if name.starts_with('.') {
                        continue;
                    }
                    // `file_type` does not follow symlinks, so a link back up the tree cannot
                    // turn the walk into a loop.
                    let file_type = entry.file_type().map_err(|source| {
                        Error::io(format!("cannot inspect {}", path.display()), source)
                    })?;
                    if file_type.is_dir() {
                        if !SKIP_DIRS.contains(&name) {
                            children.push(Work::Directory(path));
                        }
                    } else if file_type.is_file() && is_source(&path) {
                        children.push(Work::Source(path));
                    }
                }
                children.sort_by(|left, right| {
                    let left = match left {
                        Work::Directory(path) | Work::Source(path) => path,
                    };
                    let right = match right {
                        Work::Directory(path) | Work::Source(path) => path,
                    };
                    right.cmp(left)
                });
                stack.extend(children);
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

/// Whether a filesystem event names a file the authoritative project walk would discover.
///
/// Watch receives recursive events from ignored trees too. Keeping this predicate in the source
/// discovery module prevents the event fast path from silently growing a different definition of
/// the project than [`collect`]. The path need not exist: removals still need to be recognized so
/// watch can reconcile the daemon's source set.
pub(crate) fn is_discoverable(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    if !is_source(relative) {
        return false;
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => return false,
        Ok(_) => {}
        // A removal event names a path that no longer exists and still needs an authoritative
        // reconciliation. Other inspection failures are not safe to turn into source uploads.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return false,
    }
    relative
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .all(|component| {
            let std::path::Component::Normal(name) = component else {
                return false;
            };
            let Some(name) = name.to_str() else {
                return false;
            };
            !name.starts_with('.') && !SKIP_DIRS.contains(&name)
        })
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
        tree.file("src/nested/d.mts");
        tree.file("src/nested/e.cts");
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

        assert_eq!(
            found,
            vec![
                "src/a.js",
                "src/b.ts",
                "src/nested/c.tsx",
                "src/nested/d.mts",
                "src/nested/e.cts"
            ]
        );
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

    #[test]
    fn a_bounded_walk_selects_the_lexical_prefix_independent_of_creation_order() {
        let tree = Tree::new("bounded-sorted");
        for index in (0..6).rev() {
            tree.file(&format!("{index:04}.js"));
        }

        let walk = collect_with_limit(&tree.0, 5).expect("bounded walk succeeds");
        let found = walk
            .files
            .iter()
            .map(|path| {
                path.strip_prefix(&tree.0)
                    .expect("under root")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            found,
            ["0000.js", "0001.js", "0002.js", "0003.js", "0004.js"]
        );
        assert!(walk.truncated);
    }

    #[test]
    fn bounded_depth_first_order_visits_lexical_directories_before_later_siblings() {
        let tree = Tree::new("bounded-depth-first");
        tree.file("z.js");
        tree.file("b/first.js");
        tree.file("a/second.js");
        tree.file("a/first.js");

        let walk = collect_with_limit(&tree.0, 3).expect("bounded walk succeeds");
        assert_eq!(
            walk.files,
            [
                tree.0.join("a/first.js"),
                tree.0.join("a/second.js"),
                tree.0.join("b/first.js"),
            ]
        );
        assert!(walk.truncated);
    }

    #[test]
    fn event_filter_matches_discovery_even_after_a_source_is_removed() {
        let root = Path::new("/project");
        assert!(is_discoverable(root, Path::new("/project/src/main.ts")));
        assert!(is_discoverable(root, Path::new("/project/removed.js")));
        assert!(!is_discoverable(
            root,
            Path::new("/project/node_modules/pkg/index.js")
        ));
        assert!(!is_discoverable(
            root,
            Path::new("/project/.kiln/generated.js")
        ));
        assert!(!is_discoverable(root, Path::new("/other/main.js")));
        assert!(!is_discoverable(root, Path::new("/project/README.md")));
    }

    #[cfg(unix)]
    #[test]
    fn event_filter_and_walk_both_reject_symlinks_and_source_named_directories() {
        let tree = Tree::new("event-file-types");
        tree.file("real.js");
        std::os::unix::fs::symlink(tree.0.join("real.js"), tree.0.join("linked.js"))
            .expect("create source-shaped symlink");
        std::fs::create_dir(tree.0.join("fake.js")).expect("create source-shaped directory");

        let walk = collect(&tree.0).expect("walk succeeds");
        assert_eq!(walk.files, [tree.0.join("real.js")]);
        assert!(!is_discoverable(&tree.0, &tree.0.join("linked.js")));
        assert!(!is_discoverable(&tree.0, &tree.0.join("fake.js")));

        std::fs::remove_file(tree.0.join("linked.js")).expect("remove source-shaped symlink");
        assert!(
            is_discoverable(&tree.0, &tree.0.join("linked.js")),
            "a deletion event must still trigger authoritative reconciliation"
        );
    }
}
