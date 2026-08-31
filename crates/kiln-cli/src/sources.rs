//! Finding the project's source files.
//!
//! The wire protocol has no file discovery in it: the daemon is fed sources by the client
//! (`SetSource`) and never touches the filesystem. That keeps the daemon deterministic and
//! makes `--one-shot` possible at all, but it means "the project" is whatever this walk
//! finds, so the walk has to be predictable: fixed extensions, a fixed skip list, and a
//! sorted depth-first traversal so even a bounded result - and therefore every `FileId` - is
//! reproducible.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::error::{Error, Result};
use kiln_module_resolver::{NodeModuleFormat, NodePackageResolver, ResolutionKind, ResolutionPolicy};
use kiln_proto::{PackageResolution, source_digest};
use rayon::prelude::*;

const SOURCE_EXTENSIONS: [&str; 8] = ["js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts"];

/// Directories that are never project sources. Anything starting with `.` is skipped too.
const SKIP_DIRS: [&str; 5] = ["node_modules", "target", "dist", "build", "coverage"];

/// A ceiling that keeps a mistyped root (`kiln project` in `$HOME`) from turning into a
/// multi-minute walk. Hitting it is reported, never silently truncated.
const MAX_FILES: usize = 10_000;

/// The ceiling on how many sources are read at once.
///
/// Reading one is an `open` and a `read`, so the cost is kernel round trips rather than
/// arithmetic and threads past a handful only shorten the tail. Measured on the 320-module
/// `medium` workload, warm `kiln project`, 18 cores under concurrent load: 4 threads 20.6 ms,
/// 8 threads 16.7 ms, 18 threads 15.4 ms. The step from 4 to 8 is worth having; the one from 8
/// to the whole machine is not worth what it costs. That cost is the reason for a private pool
/// at all: `mbx` gives concurrent cargo invocations a single CPU budget (NOTES.md,
/// Conventions), and rayon's global pool sizes itself to the machine, so it would spend that
/// budget from inside every `kiln` invocation without being asked.
const MAX_READ_THREADS: usize = 8;

fn read_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(MAX_READ_THREADS, |cores| cores.get())
        .min(MAX_READ_THREADS)
}

pub struct Walk {
    pub files: Vec<PathBuf>,
    /// True when [`MAX_FILES`] cut the walk short.
    pub truncated: bool,
}

pub fn collect(root: &Path) -> Result<Walk> {
    collect_with_limit(root, MAX_FILES)
}

/// The reading pool, or `None` when the OS would not give us its threads - in which case the
/// caller reads serially rather than failing a whole command over a thread limit.
fn read_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(read_threads())
            .thread_name(|index| format!("kiln-source-read-{index}"))
            .build()
            .ok()
    })
    .as_ref()
}

/// Read every walked source, paired with the absolute, symlink-free path that names its
/// `FileId`, optionally substituting one already-read snapshot.
///
/// Two properties are load-bearing here and both are easy to lose.
///
/// The paths come from one `realpath` of the project root joined with each walked suffix,
/// rather than a `realpath` per file. That is only sound because [`collect_with_limit`] refuses
/// to follow symlinks: it descends entries whose `DirEntry::file_type` - an `lstat`, not a
/// `stat` - says directory, and collects only ones it says are regular files, so no walked path
/// can contain a symlinked component below the root and resolving each again is a no-op. A walk
/// that ever started following links would have to go back to resolving per file.
///
/// The reads run on [`read_threads`] threads, and `FileId` assignment must not come to depend on
/// which read finishes first. `par_iter` is indexed: every text is placed at its own position,
/// so the result is in the walk's sorted order however the threads interleave.
pub(crate) fn read_all(
    root: &Path,
    files: &[PathBuf],
    source_override: Option<(&str, &str)>,
) -> Result<Vec<(String, String)>> {
    let canonical_root = PathBuf::from(crate::commands::canonical_path(root)?);
    let paths = files
        .iter()
        .map(|file| {
            let relative = file.strip_prefix(root).map_err(|_| {
                Error::BadInput(format!(
                    "{} is not under the project root {}",
                    file.display(),
                    root.display()
                ))
            })?;
            let path = canonical_root.join(relative);
            path.to_str().map(str::to_owned).ok_or_else(|| {
                Error::BadInput(format!("path is not valid UTF-8: {}", path.display()))
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let read = |(path, file): (&String, &PathBuf)| -> Result<(String, String)> {
        let text = match source_override {
            Some((override_path, override_text)) if path == override_path => {
                override_text.to_owned()
            }
            _ => crate::commands::read_source(file)?,
        };
        Ok((path.clone(), text))
    };

    match read_pool() {
        Some(pool) => pool.install(|| paths.par_iter().zip(files).map(read).collect()),
        None => paths.iter().zip(files).map(read).collect(),
    }
}

/// Add the entry-rooted runtime dependency closure to an already-read project snapshot.
///
/// The ordinary walk intentionally skips `node_modules`; recursively walking that tree would make
/// synchronization scale with installed packages rather than with the program. This pass instead
/// parses only reachable modules and asks the same resolution-policy interface used by the daemon
/// for their exact targets. Existing project texts are reused byte-for-byte, while newly reached
/// package sources are read once and become part of the authoritative source manifest.
pub(crate) fn include_reachable_dependencies(
    root: &Path,
    entry: &str,
    texts: &mut Vec<(String, String)>,
    node: bool,
) -> Result<Vec<PackageResolution>> {
    let resolver = NodePackageResolver::new(root)
        .map_err(|error| Error::BadInput(format!("cannot resolve project modules: {error}")))?;
    let entry = PathBuf::from(entry);
    if !entry.starts_with(resolver.project_root()) {
        return Ok(Vec::new());
    }

    let mut indexes = texts
        .iter()
        .enumerate()
        .map(|(index, (path, _))| (PathBuf::from(path), index))
        .collect::<HashMap<_, _>>();
    let mut pending = vec![entry];
    let mut visited = HashSet::new();
    let mut resolutions = BTreeMap::new();

    while let Some(importer) = pending.pop() {
        if !visited.insert(importer.clone()) {
            continue;
        }
        let Some(&index) = indexes.get(&importer) else {
            return Err(Error::BadInput(format!(
                "reachable module was not captured in the source snapshot: {}",
                importer.display()
            )));
        };
        let importer_text = &texts[index].1;
        let importer_path = importer.to_str().ok_or_else(|| {
            Error::BadInput(format!("path is not valid UTF-8: {}", importer.display()))
        })?;
        let resolved_importer = kiln_module_resolver::ResolvedModule::new(
            importer.clone(),
            ResolutionKind::Relative,
        );
        let commonjs = node
            && resolver
                .node_format(&resolved_importer)
                .map_err(|error| Error::BadInput(error.to_string()))?
                == Some(NodeModuleFormat::CommonJs);
        let module = if commonjs {
            frontend_oxc::parse_commonjs(importer_path, importer_text)
        } else {
            frontend_oxc::parse(importer_path, importer_text)
        };
        let requests = if commonjs {
            kiln_hir::commonjs_literal_requires(&module)
        } else {
            module
                .runtime_requested_modules()
                .into_iter()
                .map(|request| request.specifier.clone())
                .collect()
        };

        for specifier in requests {
            if node && kiln_module_resolver::NodeBuiltin::parse(&specifier).is_some() {
                // Builtins are typed virtual graph nodes. They have no source bytes or package
                // metadata to synchronize, and the daemon's Node resolver materializes them.
                continue;
            }
            let resolved = resolver
                .resolve_with_metadata(&specifier, &importer)
                .map_err(|error| Error::BadInput(error.to_string()))?;
            let target = resolved.module.path;
            if resolved.module.kind == ResolutionKind::Package {
                let package_json = resolved.package_json.ok_or_else(|| {
                    Error::BadInput(format!(
                        "package request `{}` resolved without a package manifest",
                        specifier
                    ))
                })?;
                let package_json_text =
                    std::fs::read_to_string(&package_json).map_err(|source| {
                        Error::io(format!("cannot read {}", package_json.display()), source)
                    })?;
                let package_json = package_json.to_str().ok_or_else(|| {
                    Error::BadInput(format!(
                        "path is not valid UTF-8: {}",
                        package_json.display()
                    ))
                })?;
                resolutions.insert(
                    (importer_path.to_owned(), specifier.clone()),
                    PackageResolution {
                        importer: importer_path.to_owned(),
                        specifier: specifier.clone(),
                        target: target.to_string_lossy().into_owned(),
                        package_json: package_json.to_owned(),
                        package_json_hash: source_digest(&package_json_text),
                    },
                );
            }
            if !indexes.contains_key(&target) {
                let target_text = std::fs::read_to_string(&target).map_err(|source| {
                    Error::io(format!("cannot read {}", target.display()), source)
                })?;
                let target_path = target.to_str().ok_or_else(|| {
                    Error::BadInput(format!("path is not valid UTF-8: {}", target.display()))
                })?;
                indexes.insert(target.clone(), texts.len());
                texts.push((target_path.to_owned(), target_text));
            }
            pending.push(target);
        }
    }

    texts.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(resolutions.into_values().collect())
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
    fn adds_only_entry_reachable_package_sources() {
        let tree = Tree::new("reachable-package");
        tree.file("src/main.ts");
        tree.file("src/unrelated.ts");
        tree.file("node_modules/pkg/index.js");
        tree.file("node_modules/pkg/nested.js");
        tree.file("node_modules/pkg/unused.js");
        std::fs::write(
            tree.0.join("src/main.ts"),
            "import { value } from 'pkg'; console.log(value);\n",
        )
        .expect("write entry");
        std::fs::write(
            tree.0.join("node_modules/pkg/package.json"),
            r#"{"exports":"./index.js"}"#,
        )
        .expect("write manifest");
        std::fs::write(
            tree.0.join("node_modules/pkg/index.js"),
            "export { value } from './nested.js';\n",
        )
        .expect("write package entry");
        std::fs::write(
            tree.0.join("node_modules/pkg/nested.js"),
            "export const value = 42;\n",
        )
        .expect("write package dependency");

        let walk = collect(&tree.0).expect("walk project sources");
        let mut texts = walk
            .files
            .iter()
            .map(|path| {
                (
                    path.canonicalize()
                        .expect("canonical source")
                        .to_string_lossy()
                        .into_owned(),
                    std::fs::read_to_string(path).expect("read source"),
                )
            })
            .collect::<Vec<_>>();
        let entry = tree
            .0
            .join("src/main.ts")
            .canonicalize()
            .expect("canonical entry");

        let resolutions = include_reachable_dependencies(
            &tree.0,
            entry.to_str().expect("UTF-8 entry"),
            &mut texts,
            false,
        )
        .expect("include reachable package closure");

        let canonical_root = tree.0.canonicalize().expect("canonical root");
        let paths = texts
            .iter()
            .map(|(path, _)| {
                Path::new(path)
                    .strip_prefix(&canonical_root)
                    .expect("source under root")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [
                "node_modules/pkg/index.js",
                "node_modules/pkg/nested.js",
                "src/main.ts",
                "src/unrelated.ts",
            ]
        );
        assert_eq!(resolutions.len(), 1);
        assert_eq!(resolutions[0].importer, entry.to_string_lossy());
        assert_eq!(resolutions[0].specifier, "pkg");
        assert_eq!(
            resolutions[0].package_json,
            canonical_root
                .join("node_modules/pkg/package.json")
                .to_string_lossy()
        );
    }

    #[test]
    fn node_builtins_are_typed_graph_nodes_not_synchronized_files() {
        let tree = Tree::new("node-builtins");
        tree.file("main.mjs");
        let entry = tree.0.join("main.mjs");
        std::fs::write(
            &entry,
            "import { createServer } from 'node:http'; console.log(createServer);",
        )
        .expect("write entry");
        let canonical = entry.canonicalize().expect("canonical entry");
        let mut texts = vec![(
            canonical.to_string_lossy().into_owned(),
            std::fs::read_to_string(&entry).expect("read entry"),
        )];
        let resolutions = include_reachable_dependencies(
            &tree.0,
            canonical.to_str().expect("UTF-8 entry"),
            &mut texts,
            true,
        )
        .expect("recognized builtin needs no source snapshot");
        assert!(resolutions.is_empty());
        assert_eq!(texts.len(), 1);

        let error = include_reachable_dependencies(
            &tree.0,
            canonical.to_str().expect("UTF-8 entry"),
            &mut vec![(
                canonical.to_string_lossy().into_owned(),
                "import 'node:not-real';".into(),
            )],
            true,
        )
        .expect_err("unknown builtin must fail closed");
        assert!(error.to_string().contains("node:not-real"));
    }

    /// The loop `read_all` replaced: one `realpath` and one read per file.
    fn read_each_file_separately(files: &[PathBuf]) -> Vec<(String, String)> {
        files
            .iter()
            .map(|file| {
                (
                    crate::commands::canonical_path(file).expect("canonical source"),
                    crate::commands::read_source(file).expect("read source"),
                )
            })
            .collect()
    }

    #[test]
    fn reading_from_the_canonical_root_matches_a_realpath_per_file() {
        let tree = Tree::new("read-all-paths");
        tree.file("src/b.ts");
        tree.file("src/a.js");
        tree.file("src/nested/deep/c.tsx");
        tree.file("z.mjs");

        let walk = collect(&tree.0).expect("walk succeeds");
        assert_eq!(
            read_all(&tree.0, &walk.files, None).expect("read every source"),
            read_each_file_separately(&walk.files)
        );
    }

    /// A root reached through a symlink is the case a per-file `realpath` was really paying for.
    /// Resolving the root once has to answer the same thing.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_root_resolves_exactly_as_a_realpath_per_file_did() {
        let tree = Tree::new("read-all-symlinked-root");
        tree.file("src/a.ts");
        tree.file("src/b.ts");
        let link = tree.0.with_extension("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&tree.0, &link).expect("link the project root");

        let walk = collect(&link).expect("walk succeeds");
        assert_eq!(
            read_all(&link, &walk.files, None).expect("read every source"),
            read_each_file_separately(&walk.files)
        );
        std::fs::remove_file(&link).expect("remove the root symlink");
    }

    /// Parallel reads must not let completion order reach `FileId` assignment. A tree of files
    /// whose sizes differ by two orders of magnitude makes the reads finish out of order.
    #[test]
    fn parallel_reads_stay_in_walk_order_however_they_interleave() {
        let tree = Tree::new("read-all-order");
        for index in 0..64 {
            let path = tree.0.join(format!("src/{index:04}.ts"));
            std::fs::create_dir_all(path.parent().expect("has a parent")).expect("create dirs");
            let body = "x".repeat(if index % 8 == 0 { 200_000 } else { 20 });
            std::fs::write(&path, format!("export const v = \"{body}\";\n")).expect("write");
        }

        let walk = collect(&tree.0).expect("walk succeeds");
        let expected = read_each_file_separately(&walk.files);
        for _ in 0..8 {
            let texts = read_all(&tree.0, &walk.files, None).expect("read every source");
            assert!(
                texts.windows(2).all(|pair| pair[0].0 < pair[1].0),
                "paths came back out of order"
            );
            assert_eq!(texts, expected);
        }
    }

    #[test]
    fn an_override_replaces_only_its_own_file() {
        let tree = Tree::new("read-all-override");
        tree.file("a.ts");
        tree.file("b.ts");

        let walk = collect(&tree.0).expect("walk succeeds");
        let overridden = crate::commands::canonical_path(&tree.0.join("a.ts")).expect("canonical");
        let texts = read_all(
            &tree.0,
            &walk.files,
            Some((&overridden, "export const x = 2;\n")),
        )
        .expect("read every source");

        assert_eq!(texts[0], (overridden, "export const x = 2;\n".to_owned()));
        assert_eq!(texts[1].1, "export const x = 1;\n");
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
