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
use kiln_module_resolver::{
    NodeModuleFormat as ResolverNodeModuleFormat, NodePackageResolver, NodeRequestKind,
    ResolutionKind, ResolutionPolicy,
};
use kiln_proto::{
    NodeModuleFormat, NodeSourceFormat, PackageResolution, SourceDigest, source_digest,
};
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
#[cfg(test)]
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

/// A digest per source, reading only the files whose `stat` says they changed.
///
/// The client advertises a hash for every walked source so the daemon can answer which of them it
/// is missing (ADR 0024). Producing those hashes by reading every file is what makes a warm run
/// scale with the project rather than with the edit: on this repository reading the 449 walked
/// sources costs about 26 ms of a 54 ms floor, while `stat`-ing them costs 1.2 ms. The hash of an
/// unchanged file is already known, and `stat` is how you learn it is unchanged.
///
/// The index lives beside the artifact cache and maps a path to the `(modified, len)` it had when
/// its hash was taken. It is a cache in the strict sense: **every** disagreement means read the
/// file. A missing index, an unreadable one, a path it does not mention, a `stat` that fails, a
/// size that differs, a timestamp that differs or is unavailable -- all fall through to reading.
/// Nothing here can make a changed file look unchanged, because nothing here is trusted; the file
/// is trusted, and this only decides whether to ask it.
///
/// The failure that would matter is a file edited within one timestamp tick of its previous
/// version *and* landing on the same length. That is the classic weakness of stat-based caches,
/// and it is why the size is part of the key rather than the timestamp alone. Filesystems this
/// tool targets carry nanosecond timestamps; where the platform gives none, `modified()` fails and
/// the entry is skipped rather than guessed at.
/// A digest for every walked source, reading only what `stat` says changed.
///
/// Ordering matches the walk, exactly as [`read_all`] guarantees, because `FileId` assignment
/// keys on position and must not depend on which read finished first.
pub(crate) fn digest_all(
    root: &Path,
    files: &[PathBuf],
    source_override: Option<(&str, &str)>,
) -> Result<Vec<(String, SourceDigest)>> {
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

    /// One walked source: its path, its digest, and the `stat` that vouches for it.
    type Digested = (String, SourceDigest, Option<(u128, u64)>);
    let index = load_digest_index(root);
    let digest = |(path, file): (&String, &PathBuf)| -> Result<Digested> {
        if let Some((override_path, override_text)) = source_override {
            // A snapshot the caller already read wins over anything on disk, and is never
            // recorded: its bytes are not what the file holds.
            if path == override_path {
                return Ok((path.clone(), source_digest(override_text), None));
            }
        }
        let key = stat_key(file);
        if let (Some((modified, len)), Some((seen_modified, seen_len, hash))) =
            (key, index.get(path))
            && modified == *seen_modified
            && len == *seen_len
        {
            return Ok((path.clone(), *hash, key));
        }
        let text = crate::commands::read_source(file)?;
        Ok((path.clone(), source_digest(&text), key))
    };

    let digested: Vec<Digested> = match read_pool() {
        Some(pool) => pool.install(|| {
            paths
                .par_iter()
                .zip(files)
                .map(digest)
                .collect::<Result<Vec<_>>>()
        }),
        None => paths
            .iter()
            .zip(files)
            .map(digest)
            .collect::<Result<Vec<_>>>(),
    }?;

    let entries = digested
        .iter()
        .filter_map(|(path, hash, key)| {
            key.map(|(modified, len)| (path.clone(), modified, len, *hash))
        })
        .collect::<Vec<_>>();
    store_digest_index(root, &entries);

    Ok(digested
        .into_iter()
        .map(|(path, hash, _)| (path, hash))
        .collect())
}

fn digest_index_path(root: &Path) -> PathBuf {
    root.join(".kiln").join("source-index")
}

fn load_digest_index(root: &Path) -> HashMap<String, (u128, u64, SourceDigest)> {
    let mut index = HashMap::new();
    let Ok(text) = std::fs::read_to_string(digest_index_path(root)) else {
        return index;
    };
    for line in text.lines() {
        // modified<TAB>len<TAB>hash<TAB>path -- the path is last because it is the only field
        // that can contain a tab, and taking the remainder of the line keeps it intact.
        let mut parts = line.splitn(4, '\t');
        let (Some(modified), Some(len), Some(hash), Some(path)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (Ok(modified), Ok(len)) = (modified.parse::<u128>(), len.parse::<u64>()) else {
            continue;
        };
        let Ok(hash) = SourceDigest::from_hex(hash) else {
            continue;
        };
        index.insert(path.to_owned(), (modified, len, hash));
    }
    index
}

/// Best-effort: a write that fails costs a re-read next time and nothing else.
fn store_digest_index(root: &Path, entries: &[(String, u128, u64, SourceDigest)]) {
    let path = digest_index_path(root);
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let mut text = String::with_capacity(entries.len() * 96);
    for (file, modified, len, hash) in entries {
        text.push_str(&format!("{modified}\t{len}\t{}\t{file}\n", hash.to_hex()));
    }
    // Written through a temporary so a reader never observes a half-written index; a stale
    // temporary left by a killed process is overwritten by the next writer.
    let temporary = path.with_extension(format!("tmp.{}", std::process::id()));
    if std::fs::write(&temporary, text).is_ok() && std::fs::rename(&temporary, &path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

/// The `(modified, len)` a file has now, or `None` when the platform will not say.
fn stat_key(file: &Path) -> Option<(u128, u64)> {
    let metadata = std::fs::metadata(file).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((modified, metadata.len()))
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
) -> Result<(Vec<PackageResolution>, Vec<NodeSourceFormat>)> {
    let resolver = NodePackageResolver::new(root)
        .map_err(|error| Error::BadInput(format!("cannot resolve project modules: {error}")))?;
    let entry = PathBuf::from(entry);
    if !entry.starts_with(resolver.project_root()) {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut indexes = texts
        .iter()
        .enumerate()
        .map(|(index, (path, _))| (PathBuf::from(path), index))
        .collect::<HashMap<_, _>>();
    // The project walk skips dot-directories, so an entry inside one -- `.scratch/probe.mjs` is
    // the everyday case, since that is where the issue tracker puts working files -- is reachable
    // but absent from the snapshot. Read it here on the same terms a transitively reached module
    // gets read below. Running a file the caller named explicitly must not depend on whether the
    // walk would have volunteered it.
    if !indexes.contains_key(&entry) {
        let entry_text = std::fs::read_to_string(&entry)
            .map_err(|source| Error::io(format!("cannot read {}", entry.display()), source))?;
        let entry_path = entry.to_str().ok_or_else(|| {
            Error::BadInput(format!("path is not valid UTF-8: {}", entry.display()))
        })?;
        indexes.insert(entry.clone(), texts.len());
        texts.push((entry_path.to_owned(), entry_text));
    }

    let mut pending = vec![entry];
    let mut visited = HashSet::new();
    let mut resolutions = BTreeMap::new();
    let mut node_formats = BTreeMap::new();

    while let Some(importer) = pending.pop() {
        if !visited.insert(importer.clone()) {
            continue;
        }
        // Every module reaches `pending` only after being inserted into `indexes` -- the entry
        // just above, each import target below -- so a miss here is a broken invariant in this
        // function rather than anything the caller did. Say so, because the older wording read
        // like a complaint about the user's file and sent people looking in the wrong place.
        let Some(&index) = indexes.get(&importer) else {
            return Err(Error::BadInput(format!(
                "internal: module {} reached the work list without being read into the source \
                 snapshot; this is a bug in kiln's dependency collection, not in your project",
                importer.display()
            )));
        };
        let importer_text = &texts[index].1;
        let importer_path = importer.to_str().ok_or_else(|| {
            Error::BadInput(format!("path is not valid UTF-8: {}", importer.display()))
        })?;
        let resolved_importer =
            kiln_module_resolver::ResolvedModule::new(importer.clone(), ResolutionKind::Relative);
        let importer_format = if node {
            Some(
                resolver
                    .node_format(&resolved_importer)
                    .map_err(|error| Error::BadInput(error.to_string()))?
                    .ok_or_else(|| {
                        Error::BadInput(format!(
                            "Node resolver did not classify source {}",
                            importer.display()
                        ))
                    })?,
            )
        } else {
            None
        };
        if let Some(format) = importer_format {
            node_formats.insert(importer.clone(), to_wire_node_format(format));
        }
        let commonjs = importer_format == Some(ResolverNodeModuleFormat::CommonJs);
        let request = if commonjs {
            NodeRequestKind::Require
        } else {
            NodeRequestKind::Import
        };
        let (module, static_requires) = if commonjs {
            let (module, requires) =
                frontend_oxc::parse_commonjs_with_requires(importer_path, importer_text);
            (module, Some(requires))
        } else {
            (frontend_oxc::parse(importer_path, importer_text), None)
        };
        let requests = if commonjs {
            static_requires.expect("CommonJS facts exist")
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
            let resolved =
                match resolver.resolve_with_metadata_for_request(&specifier, &importer, request) {
                    Ok(resolved) => resolved,
                    // A CommonJS `require()` resolves when execution reaches it. Leaving an
                    // unresolved literal out of the pinned map lets the runtime throw inside the
                    // program's own `try/catch`; ESM imports remain strict graph dependencies.
                    Err(_) if commonjs => continue,
                    Err(error) => return Err(Error::BadInput(error.to_string())),
                };
            let target = resolved.module.path.clone();
            if node {
                let format = resolver
                    .node_format(&resolved.module)
                    .map_err(|error| Error::BadInput(error.to_string()))?
                    .ok_or_else(|| {
                        Error::BadInput(format!(
                            "Node resolver did not classify source {}",
                            target.display()
                        ))
                    })?;
                node_formats.insert(target.clone(), to_wire_node_format(format));
            }
            let exact_explicit = match resolved.module.kind {
                ResolutionKind::Relative => importer
                    .parent()
                    .is_some_and(|directory| directory.join(&specifier).is_file()),
                ResolutionKind::Absolute => Path::new(&specifier).is_file(),
                _ => false,
            };
            if node || !exact_explicit {
                let package_json = resolved.package_json.as_deref();
                let (package_json, package_json_hash) = if let Some(package_json) = package_json {
                    let package_json_text =
                        std::fs::read_to_string(package_json).map_err(|source| {
                            Error::io(format!("cannot read {}", package_json.display()), source)
                        })?;
                    let package_json = package_json.to_str().ok_or_else(|| {
                        Error::BadInput(format!(
                            "path is not valid UTF-8: {}",
                            package_json.display()
                        ))
                    })?;
                    (
                        Some(package_json.to_owned()),
                        Some(source_digest(&package_json_text)),
                    )
                } else {
                    (None, None)
                };
                resolutions.insert(
                    (importer_path.to_owned(), specifier.clone(), request),
                    PackageResolution {
                        importer: importer_path.to_owned(),
                        specifier: specifier.clone(),
                        target: target.to_string_lossy().into_owned(),
                        condition: match request {
                            NodeRequestKind::Import => kiln_proto::NodeRequestKind::Import,
                            NodeRequestKind::Require => kiln_proto::NodeRequestKind::Require,
                        },
                        package_json,
                        package_json_hash,
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
    Ok((
        resolutions.into_values().collect(),
        node_formats
            .into_iter()
            .map(|(path, format)| NodeSourceFormat {
                path: path.to_string_lossy().into_owned(),
                format,
            })
            .collect(),
    ))
}

fn to_wire_node_format(format: ResolverNodeModuleFormat) -> NodeModuleFormat {
    match format {
        ResolverNodeModuleFormat::EsModule => NodeModuleFormat::EsModule,
        ResolverNodeModuleFormat::CommonJs => NodeModuleFormat::CommonJs,
    }
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

        let (resolutions, node_formats) = include_reachable_dependencies(
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
            Some(
                canonical_root
                    .join("node_modules/pkg/package.json")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert!(node_formats.is_empty());
    }

    /// The project walk skips every dot-directory, so nothing under `.scratch/` is in the
    /// snapshot. Naming such a file as the entry used to fail with "reachable module was not
    /// captured in the source snapshot", which reads like a compiler bug and cost several people
    /// an afternoon each; `.scratch/` is where `docs/agents/issue-tracker.md` tells them to work.
    #[test]
    fn an_entry_inside_a_dot_directory_runs_with_its_own_neighbours() {
        let tree = Tree::new("dot-directory-entry");
        tree.file("src/unrelated.ts");
        tree.file(".scratch/probe/dep.mjs");
        tree.file(".scratch/probe/main.mjs");
        std::fs::write(
            tree.0.join(".scratch/probe/dep.mjs"),
            "export const value = 42;\n",
        )
        .expect("write scratch dependency");
        std::fs::write(
            tree.0.join(".scratch/probe/main.mjs"),
            "import { value } from './dep.mjs'; console.log(value);\n",
        )
        .expect("write scratch entry");

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
        // Precondition: the walk really does not volunteer the scratch tree, so the assertion
        // below is testing the entry fallback rather than an accidentally-complete snapshot.
        assert!(
            texts.iter().all(|(path, _)| !path.contains(".scratch")),
            "the walk was expected to skip .scratch/, but it collected {texts:?}"
        );

        let entry = tree
            .0
            .join(".scratch/probe/main.mjs")
            .canonicalize()
            .expect("canonical entry");
        include_reachable_dependencies(
            &tree.0,
            entry.to_str().expect("UTF-8 entry"),
            &mut texts,
            false,
        )
        .expect("an entry under a dot-directory is runnable");

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
                ".scratch/probe/dep.mjs",
                ".scratch/probe/main.mjs",
                "src/unrelated.ts",
            ],
            "the entry and the modules it imports must both reach the snapshot"
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
        let (resolutions, node_formats) = include_reachable_dependencies(
            &tree.0,
            canonical.to_str().expect("UTF-8 entry"),
            &mut texts,
            true,
        )
        .expect("recognized builtin needs no source snapshot");
        assert!(resolutions.is_empty());
        assert_eq!(
            node_formats,
            vec![NodeSourceFormat {
                path: canonical.to_string_lossy().into_owned(),
                format: NodeModuleFormat::EsModule,
            }]
        );
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

    #[test]
    fn node_commonjs_missing_require_is_left_for_runtime_catch() {
        let tree = Tree::new("commonjs-optional-require");
        tree.file("main.cjs");
        let entry = tree.0.join("main.cjs");
        std::fs::write(
            &entry,
            "try { require('not-installed'); } catch { console.log('caught'); }",
        )
        .expect("write entry");
        let canonical = entry.canonicalize().expect("canonical entry");
        let mut texts = vec![(
            canonical.to_string_lossy().into_owned(),
            std::fs::read_to_string(&entry).expect("read entry"),
        )];

        let (resolutions, node_formats) = include_reachable_dependencies(
            &tree.0,
            canonical.to_str().expect("UTF-8 entry"),
            &mut texts,
            true,
        )
        .expect("missing CommonJS target is runtime work");

        assert!(resolutions.is_empty());
        assert_eq!(node_formats.len(), 1);
        assert_eq!(texts.len(), 1);
    }

    #[test]
    fn node_resolves_typescript_extension_alias_imports() {
        let tree = Tree::new("ts-extension-alias");
        tree.file("package.json");
        std::fs::write(tree.0.join("package.json"), r#"{"type":"module"}"#)
            .expect("write package.json");
        tree.file("src/main.ts");
        tree.file("src/dep.ts");
        let entry = tree.0.join("src/main.ts");
        let dep = tree.0.join("src/dep.ts");
        std::fs::write(
            &entry,
            "import { answer } from './dep.js'; console.log(answer);",
        )
        .expect("write entry");
        std::fs::write(&dep, "export const answer = 42;").expect("write dep");

        let canonical_entry = entry.canonicalize().expect("canonical entry");
        let canonical_dep = dep.canonicalize().expect("canonical dep");
        let mut texts = vec![(
            canonical_entry.to_string_lossy().into_owned(),
            std::fs::read_to_string(&entry).expect("read entry"),
        )];

        let (resolutions, node_formats) = include_reachable_dependencies(
            &tree.0,
            canonical_entry.to_str().expect("UTF-8 entry"),
            &mut texts,
            true,
        )
        .expect("resolves .js to .ts under node policy");

        assert_eq!(resolutions.len(), 1);
        assert_eq!(resolutions[0].specifier, "./dep.js");
        assert_eq!(resolutions[0].target, canonical_dep.to_string_lossy());
        assert_eq!(texts.len(), 2);
        assert_eq!(node_formats.len(), 2);
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
