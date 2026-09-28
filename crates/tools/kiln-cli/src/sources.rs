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
    ResolutionKind, ResolutionPolicy, is_asset_extension,
};
use kiln_proto::{
    DeclaredInclude, NodeModuleFormat, NodeSourceFormat, PackageResolution, SourceDigest,
    source_digest,
};

/// The root package's explicit closed-world module declaration (ADR 0067 §2). A relative
/// spelling resolves from this package boundary, even when the command's entry is in `src/`.
fn declared_include_specifiers(root: &Path) -> Result<Vec<String>> {
    let manifest = root.join("package.json");
    if !manifest.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&manifest)
        .map_err(|source| Error::io(format!("cannot read {}", manifest.display()), source))?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        Error::BadInput(format!("cannot parse {}: {error}", manifest.display()))
    })?;
    let Some(kiln) = value.get("kiln") else {
        return Ok(Vec::new());
    };
    let Some(kiln) = kiln.as_object() else {
        return Err(Error::BadInput(format!(
            "{}: `kiln` must be an object with an `include` array",
            manifest.display()
        )));
    };
    let Some(include) = kiln.get("include") else {
        return Ok(Vec::new());
    };
    let Some(include) = include.as_array() else {
        return Err(Error::BadInput(format!(
            "{}: `kiln.include` must be an array of module specifiers",
            manifest.display()
        )));
    };
    let mut specifiers = Vec::with_capacity(include.len());
    for (index, value) in include.iter().enumerate() {
        let Some(specifier) = value.as_str().filter(|value| !value.is_empty()) else {
            return Err(Error::BadInput(format!(
                "{}: `kiln.include[{index}]` must be a nonempty module specifier",
                manifest.display()
            )));
        };
        specifiers.push(specifier.to_owned());
    }
    specifiers.sort();
    if let Some(pair) = specifiers.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(Error::BadInput(format!(
            "{}: duplicate `kiln.include` specifier {:?}",
            manifest.display(),
            pair[0]
        )));
    }
    Ok(specifiers)
}

fn captured_importer_path(raw: &str) -> Result<PathBuf> {
    if raw.starts_with("file:") {
        let url = url::Url::parse(raw)
            .map_err(|error| Error::BadInput(format!("invalid file URL `{raw}`: {error}")))?;
        return url.to_file_path().map_err(|()| {
            Error::BadInput(format!("file URL does not identify a local path: {raw}"))
        });
    }
    Ok(PathBuf::from(raw))
}
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
    let cores = std::thread::available_parallelism().map_or(MAX_READ_THREADS, |cores| cores.get());
    let kiln_jobs = std::env::var("KILN_JOBS").ok();
    let rayon_threads = std::env::var("RAYON_NUM_THREADS").ok();
    bounded_threads(
        MAX_READ_THREADS,
        cores,
        [kiln_jobs.as_deref(), rayon_threads.as_deref()],
    )
}

/// Bound this private pool by its own ceiling, the available CPU count, and any positive caller
/// limits. This is a per-pool limit, not a process-wide CPU guarantee.
fn bounded_threads(max_threads: usize, cores: usize, requested: [Option<&str>; 2]) -> usize {
    let mut threads = max_threads.max(1).min(cores.max(1));
    for count in requested
        .into_iter()
        .flatten()
        .filter_map(|value| value.parse::<usize>().ok())
        .filter(|count| *count > 0)
    {
        threads = threads.min(count);
    }
    threads
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

    // Do not start a per-process Rayon pool to read one source. Snapshot substitution and
    // indexed order are identical on this serial path.
    if files.len() <= 1 {
        return paths.iter().zip(files).map(read).collect();
    }
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

    // A one-source command has no parallel read work. Avoid starting the source-read pool for one
    // digest, which only creates workers the process exits with immediately.
    let digested: Vec<Digested> = if files.len() <= 1 {
        paths
            .iter()
            .zip(files)
            .map(digest)
            .collect::<Result<Vec<_>>>()?
    } else {
        match read_pool() {
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
        }?
    };

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
    let (resolutions, formats, _) =
        include_reachable_dependencies_with_includes(root, entry, texts, node)?;
    Ok((resolutions, formats))
}

pub(crate) fn include_reachable_dependencies_with_includes(
    root: &Path,
    entry: &str,
    texts: &mut Vec<(String, String)>,
    node: bool,
) -> Result<(
    Vec<PackageResolution>,
    Vec<NodeSourceFormat>,
    Vec<DeclaredInclude>,
)> {
    let resolver = NodePackageResolver::new(root)
        .map_err(|error| Error::BadInput(format!("cannot resolve project modules: {error}")))?;
    let entry = PathBuf::from(entry);
    if !entry.starts_with(resolver.project_root()) {
        return Ok((Vec::new(), Vec::new(), Vec::new()));
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

    // Static imports from the entry are strict graph roots. Literal dynamic imports and
    // CommonJS requires are runtime-reachable subtrees: capture anything that exists, but do not
    // reject the whole snapshot when a descendant is unavailable.
    let mut pending = vec![(entry, true)];
    let mut declared_includes = Vec::new();
    if node {
        let package_importer = resolver.project_root().join("package.json");
        for specifier in declared_include_specifiers(resolver.project_root())? {
            let resolved = resolver
                .resolve_with_metadata_for_request(
                    &specifier,
                    &package_importer,
                    NodeRequestKind::Require,
                )
                .map_err(|error| {
                    Error::BadInput(format!(
                        "{}: `kiln.include` cannot resolve {:?}: {error}",
                        package_importer.display(),
                        specifier
                    ))
                })?;
            let target = resolved.module.path;
            if target
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(is_asset_extension)
            {
                return Err(Error::BadInput(format!(
                    "{}: `kiln.include` names an asset, not a module: {:?}",
                    package_importer.display(),
                    specifier
                )));
            }
            if !indexes.contains_key(&target) {
                let text = std::fs::read_to_string(&target).map_err(|source| {
                    Error::io(
                        format!("cannot read declared module {}", target.display()),
                        source,
                    )
                })?;
                let path = target.to_str().ok_or_else(|| {
                    Error::BadInput(format!("path is not valid UTF-8: {}", target.display()))
                })?;
                indexes.insert(target.clone(), texts.len());
                texts.push((path.to_owned(), text));
            }
            declared_includes.push(DeclaredInclude {
                specifier,
                target: target.to_string_lossy().into_owned(),
            });
            pending.push((target, true));
        }
    }
    // Keep the strongest reachability mode seen for each module. A dynamic edge may be popped
    // before a later static edge to the same path; the latter must still validate its subtree.
    let mut visited = HashMap::<PathBuf, bool>::new();
    let mut resolutions = BTreeMap::new();
    let mut node_formats = BTreeMap::new();

    while let Some((importer, strict)) = pending.pop() {
        match visited.get(&importer).copied() {
            Some(previous_strict) if previous_strict || !strict => continue,
            Some(_) | None => {
                visited.insert(importer.clone(), strict);
            }
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
        if importer
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(kiln_data_formats::is_data_extension)
        {
            // Data files (JSON, JSON5, JSONC, YAML, TOML) are leaf nodes in the module graph.
            continue;
        }
        let commonjs = importer_format == Some(ResolverNodeModuleFormat::CommonJs);
        let (module, static_requires) = if commonjs {
            let (module, requires) =
                kiln_frontend_oxc::parse_commonjs_with_requires(importer_path, importer_text);
            (module, Some(requires))
        } else {
            (kiln_frontend_oxc::parse(importer_path, importer_text), None)
        };
        let mut requests: Vec<(String, NodeRequestKind, bool, PathBuf)> = if commonjs {
            static_requires
                .expect("CommonJS facts exist")
                .into_iter()
                .map(|specifier| (specifier, NodeRequestKind::Require, true, importer.clone()))
                .collect()
        } else {
            module
                .runtime_requested_modules()
                .into_iter()
                .map(|request| {
                    (
                        request.specifier.clone(),
                        NodeRequestKind::Import,
                        true,
                        importer.clone(),
                    )
                })
                .collect()
        };
        // Literal createRequire/getBuiltinModule calls are lazy at runtime, but a relative
        // createRequire target still needs its source text in the synchronized snapshot.
        for runtime in
            kiln_frontend_oxc::discover_node_runtime_requests(importer_path, importer_text)
        {
            let runtime_importer = captured_importer_path(&runtime.importer)?;
            if !requests
                .iter()
                .any(|(existing, kind, _, existing_importer)| {
                    existing == &runtime.specifier
                        && *kind == NodeRequestKind::Require
                        && existing_importer == &runtime_importer
                })
            {
                requests.push((
                    runtime.specifier,
                    NodeRequestKind::Require,
                    false,
                    runtime_importer,
                ));
            }
        }
        // Dynamic imports with a literal target are graph edges too.  The compiler records
        // these separately from static import declarations because their evaluation is deferred,
        // but source synchronization still has to capture the complete reachable closure.  If we
        // omit them, native graph construction can discover a downstream relative import whose
        // source and pinned Node format were never sent by the client.
        for target in kiln_ir::collect_dynamic_imports(&module.body) {
            if let kiln_ir::DynamicImportTarget::Static(specifier) = target
                && !requests.iter().any(|(existing, kind, _, _)| {
                    existing == &specifier && *kind == NodeRequestKind::Import
                })
            {
                // Dynamic import always uses the import condition, even in a CommonJS module;
                // a missing target remains a deferred runtime rejection rather than a build error.
                requests.push((specifier, NodeRequestKind::Import, false, importer.clone()));
            }
        }

        for (specifier, request, required, request_importer) in requests {
            if node && kiln_module_resolver::NodeBuiltin::parse(&specifier).is_some() {
                // Builtins are typed virtual graph nodes. They have no source bytes or package
                // metadata to synchronize, and the daemon's Node resolver materializes them.
                continue;
            }
            let resolved = match resolver.resolve_with_metadata_for_request(
                &specifier,
                &request_importer,
                request,
            ) {
                Ok(resolved) => resolved,
                // A CommonJS `require()` resolves when execution reaches it. Leaving an
                // unresolved literal out of the pinned map lets the runtime throw inside the
                // program's own `try/catch`; ESM imports remain strict graph dependencies.
                Err(_) if commonjs || !required || !strict => continue,
                Err(error) => return Err(Error::BadInput(error.to_string())),
            };
            let target = resolved.module.path.clone();
            let asset = target
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(is_asset_extension);
            if node && !asset {
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
                ResolutionKind::Relative => request_importer
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
            // Binary assets are explicit path modules.  They belong in the resolution map so the
            // graph can materialize their path, but are not UTF-8 sources and must not enter the
            // source snapshot or dependency work list.
            if asset {
                continue;
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
            pending.push((target, strict && required && !commonjs));
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
        declared_includes,
    ))
}

fn to_wire_node_format(format: ResolverNodeModuleFormat) -> NodeModuleFormat {
    match format {
        ResolverNodeModuleFormat::EsModule => NodeModuleFormat::EsModule,
        ResolverNodeModuleFormat::CommonJs => NodeModuleFormat::CommonJs,
    }
}

/// Audit reachable package resolutions against the root `package.json`.
/// Emits diagnostic notes for "phantom dependencies" (packages imported by project code
/// that resolved from `node_modules` but are not declared in root `package.json`).
fn read_declared_deps(manifest_path: &Path) -> HashSet<String> {
    let mut declared = HashSet::new();
    let Ok(manifest_bytes) = std::fs::read(manifest_path) else {
        return declared;
    };
    let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&manifest_bytes) else {
        return declared;
    };
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(obj) = manifest.get(section).and_then(|v| v.as_object()) {
            for key in obj.keys() {
                declared.insert(key.clone());
            }
        }
    }
    declared
}

pub(crate) fn audit_phantom_dependencies(
    project_root: &Path,
    resolutions: &[PackageResolution],
) -> Vec<String> {
    let mut manifest_cache: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    let root_manifest_path = project_root.join("package.json");
    let root_declared = read_declared_deps(&root_manifest_path);
    manifest_cache.insert(project_root.to_path_buf(), root_declared);

    let mut notes = Vec::new();
    let mut seen = HashSet::new();

    for res in resolutions {
        let specifier = &res.specifier;
        if specifier.starts_with('.')
            || specifier.starts_with('/')
            || specifier.starts_with('#')
            || kiln_module_resolver::NodeBuiltin::parse(specifier).is_some()
        {
            continue;
        }

        let importer_path = Path::new(&res.importer);
        if importer_path
            .components()
            .any(|c| c.as_os_str() == "node_modules")
        {
            continue;
        }

        let pkg_name = if specifier.starts_with('@') {
            let mut parts = specifier.split('/');
            let scope = parts.next().unwrap_or("");
            let name = parts.next().unwrap_or("");
            if scope.is_empty() || name.is_empty() {
                continue;
            }
            format!("{scope}/{name}")
        } else {
            specifier.split('/').next().unwrap_or("").to_string()
        };

        if pkg_name.is_empty() {
            continue;
        }

        let target_path = Path::new(&res.target);
        let is_in_node_modules = target_path
            .components()
            .any(|c| c.as_os_str() == "node_modules");

        if !is_in_node_modules {
            continue;
        }

        let mut is_declared = false;
        let mut current = importer_path.parent();
        while let Some(dir) = current {
            let deps = manifest_cache.entry(dir.to_path_buf()).or_insert_with(|| {
                let p = dir.join("package.json");
                if p.is_file() {
                    read_declared_deps(&p)
                } else {
                    HashSet::new()
                }
            });
            if deps.contains(&pkg_name) {
                is_declared = true;
                break;
            }
            if dir == project_root {
                break;
            }
            current = dir.parent();
        }

        if !is_declared && seen.insert((pkg_name.clone(), res.importer.clone())) {
            let display_importer = importer_path
                .strip_prefix(project_root)
                .unwrap_or(importer_path)
                .display();
            notes.push(format!(
                "phantom dependency: '{pkg_name}' is imported by '{display_importer}' but not declared in package.json"
            ));
        }
    }

    notes
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

    #[test]
    fn source_read_pool_honors_positive_caller_limits() {
        assert_eq!(bounded_threads(MAX_READ_THREADS, 32, [None, None]), 8);
        assert_eq!(bounded_threads(MAX_READ_THREADS, 2, [Some("16"), None]), 2);
        assert_eq!(
            bounded_threads(MAX_READ_THREADS, 32, [Some("16"), Some("1")]),
            1
        );
        assert_eq!(
            bounded_threads(MAX_READ_THREADS, 32, [Some("2"), Some("4")]),
            2
        );
    }

    #[test]
    fn source_read_pool_ignores_invalid_and_nonpositive_limits() {
        for value in [Some("0"), Some("-1"), Some("nope")] {
            assert_eq!(bounded_threads(MAX_READ_THREADS, 32, [value, None]), 8);
        }
        assert_eq!(bounded_threads(MAX_READ_THREADS, 0, [None, None]), 1);
        assert_eq!(
            bounded_threads(MAX_READ_THREADS, 32, [Some("9999"), None]),
            8
        );
    }

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
    fn declared_include_is_a_lazy_source_root_from_the_package_boundary() {
        let tree = Tree::new("declared-include");
        tree.file("src/main.cjs");
        tree.file("worker-task.cjs");
        tree.file("worker-dependency.cjs");
        std::fs::write(
            tree.0.join("package.json"),
            r#"{"kiln":{"include":["./worker-task.cjs"]}}"#,
        )
        .expect("write include declaration");
        std::fs::write(
            tree.0.join("worker-task.cjs"),
            "module.exports = require('./worker-dependency.cjs');",
        )
        .expect("write included worker");
        let root = tree.0.canonicalize().expect("canonical root");
        let entry = root.join("src/main.cjs");
        let mut texts = Vec::new();
        let (resolutions, formats, includes) = include_reachable_dependencies_with_includes(
            &root,
            entry.to_str().expect("UTF-8 path"),
            &mut texts,
            true,
        )
        .expect("collect declared closure");
        assert_eq!(includes.len(), 1);
        assert_eq!(includes[0].specifier, "./worker-task.cjs");
        assert_eq!(
            includes[0].target,
            root.join("worker-task.cjs").to_string_lossy()
        );
        assert!(texts.iter().any(|(path, _)| path == &includes[0].target));
        assert!(
            texts
                .iter()
                .any(|(path, _)| path == &root.join("worker-dependency.cjs").to_string_lossy())
        );
        assert!(
            formats
                .iter()
                .any(|format| format.path == includes[0].target)
        );
        assert!(
            resolutions
                .iter()
                .any(|resolution| resolution.specifier == "./worker-dependency.cjs")
        );
    }

    #[test]
    fn invalid_declared_include_names_its_package_and_entry() {
        let tree = Tree::new("invalid-declared-include");
        tree.file("main.cjs");
        std::fs::write(
            tree.0.join("package.json"),
            r#"{"kiln":{"include":["./missing.cjs"]}}"#,
        )
        .expect("write include declaration");
        let root = tree.0.canonicalize().expect("canonical root");
        let entry = root.join("main.cjs");
        let error = include_reachable_dependencies_with_includes(
            &root,
            entry.to_str().expect("UTF-8 path"),
            &mut Vec::new(),
            true,
        )
        .expect_err("missing declared module");
        let message = error.to_string();
        assert!(message.contains("package.json"), "{message}");
        assert!(message.contains("kiln.include"), "{message}");
        assert!(message.contains("./missing.cjs"), "{message}");
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

    #[test]
    fn captures_literal_dynamic_import_closure_and_formats() {
        let tree = Tree::new("reachable-dynamic-package");
        tree.file("src/main.mjs");
        tree.file("node_modules/pkg/index.mjs");
        tree.file("node_modules/pkg/nested.mjs");
        tree.file("node_modules/pkg/leaf.mjs");
        std::fs::write(
            tree.0.join("src/main.mjs"),
            "import('pkg').then(console.log);\n",
        )
        .expect("write entry");
        std::fs::write(
            tree.0.join("node_modules/pkg/package.json"),
            r#"{"exports":"./index.mjs","type":"module"}"#,
        )
        .expect("write manifest");
        std::fs::write(
            tree.0.join("node_modules/pkg/index.mjs"),
            "export { value } from './nested.mjs';\n",
        )
        .expect("write package entry");
        std::fs::write(
            tree.0.join("node_modules/pkg/nested.mjs"),
            "export { value } from './leaf.mjs';\n",
        )
        .expect("write nested module");
        std::fs::write(
            tree.0.join("node_modules/pkg/leaf.mjs"),
            "export const value = 42;\n",
        )
        .expect("write leaf module");

        let entry = tree.0.join("src/main.mjs").canonicalize().expect("entry");
        let mut texts = vec![(
            entry.to_string_lossy().into_owned(),
            std::fs::read_to_string(&entry).expect("read entry"),
        )];
        let (resolutions, formats) = include_reachable_dependencies(
            &tree.0,
            entry.to_str().expect("UTF-8 entry"),
            &mut texts,
            true,
        )
        .expect("capture dynamic closure");

        assert_eq!(resolutions.len(), 3);
        assert_eq!(texts.len(), 4);
        assert_eq!(formats.len(), 4);
    }

    #[test]
    fn leaves_binary_asset_imports_out_of_utf8_source_snapshot() {
        let tree = Tree::new("reachable-binary-asset");
        tree.file("src/main.mjs");
        let entry = tree.0.join("src/main.mjs");
        let asset = tree.0.join("src/tree-sitter.wasm");
        std::fs::write(
            &entry,
            "import wasm from './tree-sitter.wasm'; console.log(wasm);\n",
        )
        .expect("write entry");
        std::fs::write(&asset, [0_u8, 159, 146, 150]).expect("write binary asset");

        let entry = entry.canonicalize().expect("entry");
        let mut texts = vec![(
            entry.to_string_lossy().into_owned(),
            std::fs::read_to_string(&entry).expect("read entry"),
        )];
        let (resolutions, formats) = include_reachable_dependencies(
            &tree.0,
            entry.to_str().expect("UTF-8 entry"),
            &mut texts,
            true,
        )
        .expect("capture asset edge");

        assert_eq!(resolutions.len(), 1);
        assert_eq!(
            resolutions[0].target,
            asset.canonicalize().unwrap().to_string_lossy()
        );
        assert_eq!(texts.len(), 1);
        assert_eq!(formats.len(), 1);
    }

    #[test]
    fn leaves_missing_literal_dynamic_import_deferred() {
        let tree = Tree::new("missing-dynamic-import");
        tree.file("src/main.mjs");
        tree.file("src/optional.mjs");
        let entry = tree.0.join("src/main.mjs");
        std::fs::write(&entry, "import('./optional.mjs').catch(() => {});\n").expect("write entry");
        std::fs::write(
            tree.0.join("src/optional.mjs"),
            "import './missing.mjs'; export const optional = true;\n",
        )
        .expect("write optional module");
        let entry = entry.canonicalize().expect("entry");
        let mut texts = vec![(
            entry.to_string_lossy().into_owned(),
            std::fs::read_to_string(&entry).expect("read entry"),
        )];
        let (resolutions, formats) = include_reachable_dependencies(
            &tree.0,
            entry.to_str().expect("UTF-8 entry"),
            &mut texts,
            true,
        )
        .expect("missing dynamic subtree dependency stays deferred");
        assert_eq!(resolutions.len(), 1);
        assert_eq!(resolutions[0].specifier, "./optional.mjs");
        assert_eq!(texts.len(), 2);
        assert_eq!(formats.len(), 2);
    }

    #[test]
    fn missing_static_import_remains_strict() {
        let tree = Tree::new("missing-static-import");
        tree.file("src/main.mjs");
        let entry = tree.0.join("src/main.mjs");
        std::fs::write(&entry, "import './missing.mjs';\n").expect("write entry");
        let entry = entry.canonicalize().expect("entry");
        let mut texts = vec![(
            entry.to_string_lossy().into_owned(),
            std::fs::read_to_string(&entry).expect("read entry"),
        )];

        let error = include_reachable_dependencies(
            &tree.0,
            entry.to_str().expect("UTF-8 entry"),
            &mut texts,
            true,
        )
        .expect_err("missing static target must fail the snapshot");
        assert!(error.to_string().contains("missing.mjs"));
    }

    #[test]
    fn a_shared_module_reached_dynamically_then_statically_remains_strict() {
        let tree = Tree::new("shared-static-and-dynamic");
        tree.file("src/main.mjs");
        tree.file("src/strict.mjs");
        tree.file("src/shared.mjs");
        let entry = tree.0.join("src/main.mjs");
        std::fs::write(
            &entry,
            "import './strict.mjs';\nimport('./shared.mjs').catch(() => {});\n",
        )
        .expect("write entry");
        std::fs::write(tree.0.join("src/strict.mjs"), "import './shared.mjs';\n")
            .expect("write strict importer");
        std::fs::write(tree.0.join("src/shared.mjs"), "import './missing.mjs';\n")
            .expect("write shared importer");

        let entry = entry.canonicalize().expect("entry");
        let mut texts = vec![(
            entry.to_string_lossy().into_owned(),
            std::fs::read_to_string(&entry).expect("read entry"),
        )];
        let error = include_reachable_dependencies(
            &tree.0,
            entry.to_str().expect("UTF-8 entry"),
            &mut texts,
            true,
        )
        .expect_err("the later static edge must make the shared subtree strict");
        assert!(error.to_string().contains("missing.mjs"));
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
    fn node_commonjs_conditional_require_and_import_use_export_conditions() {
        let tree = Tree::new("commonjs-export-conditions");
        tree.file("main.cjs");
        tree.file("node_modules/pkg/package.json");
        tree.file("node_modules/pkg/cjs.cjs");
        tree.file("node_modules/pkg/esm.mjs");
        std::fs::write(
            tree.0.join("main.cjs"),
            "const cjs = require('pkg');\nif (false) import('pkg');\n",
        )
        .expect("write entry");
        std::fs::write(
            tree.0.join("node_modules/pkg/package.json"),
            r#"{"exports":{"import":"./esm.mjs","require":"./cjs.cjs"}}"#,
        )
        .expect("write package manifest");
        std::fs::write(
            tree.0.join("node_modules/pkg/cjs.cjs"),
            "module.exports = 1;\n",
        )
        .expect("write CommonJS target");
        std::fs::write(
            tree.0.join("node_modules/pkg/esm.mjs"),
            "export default 2;\n",
        )
        .expect("write ESM target");

        let entry = tree.0.join("main.cjs").canonicalize().expect("entry");
        let mut texts = vec![(
            entry.to_string_lossy().into_owned(),
            std::fs::read_to_string(&entry).expect("read entry"),
        )];
        let (resolutions, formats) = include_reachable_dependencies(
            &tree.0,
            entry.to_str().expect("UTF-8 entry"),
            &mut texts,
            true,
        )
        .expect("both package conditions resolve");

        assert_eq!(resolutions.len(), 2);
        assert!(resolutions.iter().any(|resolution| resolution.condition
            == kiln_proto::NodeRequestKind::Require
            && resolution.target.ends_with("/node_modules/pkg/cjs.cjs")));
        assert!(resolutions.iter().any(|resolution| resolution.condition
            == kiln_proto::NodeRequestKind::Import
            && resolution.target.ends_with("/node_modules/pkg/esm.mjs")));
        assert_eq!(texts.len(), 3);
        assert_eq!(formats.len(), 3);
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

    #[test]
    fn node_resolves_tsconfig_paths_imports() {
        let tree = Tree::new("tsconfig-paths");
        tree.file("package.json");
        std::fs::write(tree.0.join("package.json"), r#"{"type":"module"}"#)
            .expect("write package.json");
        tree.file("tsconfig.json");
        std::fs::write(
            tree.0.join("tsconfig.json"),
            r#"{
                "compilerOptions": {
                    "baseUrl": ".",
                    "paths": {
                        "@/*": ["src/*"]
                    }
                }
            }"#,
        )
        .expect("write tsconfig.json");
        tree.file("src/main.ts");
        tree.file("src/utils/math.ts");
        let entry = tree.0.join("src/main.ts");
        let math = tree.0.join("src/utils/math.ts");
        std::fs::write(
            &entry,
            "import { answer } from '@/utils/math'; console.log(answer);",
        )
        .expect("write entry");
        std::fs::write(&math, "export const answer = 42;").expect("write math");

        let canonical_entry = entry.canonicalize().expect("canonical entry");
        let canonical_math = math.canonicalize().expect("canonical math");
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
        .expect("resolves tsconfig path alias under node policy");

        assert_eq!(resolutions.len(), 1);
        assert_eq!(resolutions[0].specifier, "@/utils/math");
        assert_eq!(resolutions[0].target, canonical_math.to_string_lossy());
        assert_eq!(texts.len(), 2);
        assert_eq!(node_formats.len(), 2);
    }

    #[test]
    fn node_resolves_and_loads_data_formats_imports() {
        let tree = Tree::new("data-formats-imports");
        tree.file("package.json");
        std::fs::write(
            tree.0.join("package.json"),
            r#"{"name": "app", "type": "module"}"#,
        )
        .expect("write package.json");

        tree.file("src/main.ts");
        tree.file("data/config.yaml");
        tree.file("data/settings.toml");

        let entry = tree.0.join("src/main.ts");
        let yaml = tree.0.join("data/config.yaml");
        let toml = tree.0.join("data/settings.toml");

        std::fs::write(
            &entry,
            "import config from '../data/config.yaml';\nimport settings from '../data/settings.toml';\nconsole.log(config, settings);\n",
        )
        .expect("write entry");
        std::fs::write(&yaml, "name: kiln\nport: 8080\n").expect("write yaml");
        std::fs::write(&toml, "[database]\nhost = \"localhost\"\n").expect("write toml");

        let canonical_entry = entry.canonicalize().expect("canonical entry");
        let canonical_yaml = yaml.canonicalize().expect("canonical yaml");
        let canonical_toml = toml.canonicalize().expect("canonical toml");

        let mut texts = vec![(
            canonical_entry.to_string_lossy().into_owned(),
            std::fs::read_to_string(&entry).expect("read entry"),
        )];

        let (resolutions, _) = include_reachable_dependencies(
            &tree.0,
            canonical_entry.to_str().expect("UTF-8 entry"),
            &mut texts,
            true,
        )
        .expect("resolves yaml and toml imports without attempting to parse as JS");

        assert_eq!(resolutions.len(), 2);
        let targets: HashSet<_> = resolutions.into_iter().map(|r| r.target).collect();
        assert!(targets.contains(canonical_yaml.to_str().unwrap()));
        assert!(targets.contains(canonical_toml.to_str().unwrap()));
        assert_eq!(texts.len(), 3);
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
    fn one_file_reads_and_digests_keep_the_source_snapshot() {
        let tree = Tree::new("single-source-snapshot");
        tree.file("main.ts");
        let walk = collect(&tree.0).expect("walk succeeds");
        let path = crate::commands::canonical_path(&tree.0.join("main.ts")).expect("canonical");
        let snapshot = "export const x = 2;\n";

        let texts = read_all(&tree.0, &walk.files, Some((&path, snapshot)))
            .expect("read the source snapshot");
        assert_eq!(texts, vec![(path.clone(), snapshot.to_owned())]);

        let digests = digest_all(&tree.0, &walk.files, Some((&path, snapshot)))
            .expect("digest the source snapshot");
        assert_eq!(digests, vec![(path, source_digest(snapshot))]);
        assert!(
            read_all(&tree.0, &[], None)
                .expect("empty read is empty")
                .is_empty()
        );
        assert!(
            digest_all(&tree.0, &[], None)
                .expect("empty digest is empty")
                .is_empty()
        );
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

    #[test]
    fn audits_phantom_dependencies_not_declared_in_package_json() {
        let tree = Tree::new("phantom-deps-audit");
        tree.file("package.json");
        std::fs::write(
            tree.0.join("package.json"),
            r#"{
                "name": "my-app",
                "dependencies": {
                    "declared-pkg": "^1.0.0"
                }
            }"#,
        )
        .expect("write package.json");

        let resolutions = vec![
            PackageResolution {
                importer: tree.0.join("src/index.ts").to_string_lossy().into_owned(),
                specifier: "declared-pkg".to_string(),
                target: tree
                    .0
                    .join("node_modules/declared-pkg/index.js")
                    .to_string_lossy()
                    .into_owned(),
                condition: kiln_proto::NodeRequestKind::Import,
                package_json: None,
                package_json_hash: None,
            },
            PackageResolution {
                importer: tree.0.join("src/index.ts").to_string_lossy().into_owned(),
                specifier: "phantom-pkg".to_string(),
                target: tree
                    .0
                    .join("node_modules/phantom-pkg/index.js")
                    .to_string_lossy()
                    .into_owned(),
                condition: kiln_proto::NodeRequestKind::Import,
                package_json: None,
                package_json_hash: None,
            },
            PackageResolution {
                importer: tree.0.join("src/index.ts").to_string_lossy().into_owned(),
                specifier: "@scoped/phantom".to_string(),
                target: tree
                    .0
                    .join("node_modules/@scoped/phantom/index.js")
                    .to_string_lossy()
                    .into_owned(),
                condition: kiln_proto::NodeRequestKind::Import,
                package_json: None,
                package_json_hash: None,
            },
        ];

        let notes = audit_phantom_dependencies(&tree.0, &resolutions);
        assert_eq!(notes.len(), 2);
        assert!(notes[0].contains("phantom dependency: 'phantom-pkg'"));
        assert!(notes[1].contains("phantom dependency: '@scoped/phantom'"));
    }

    #[test]
    fn debug_pi_reachable_dependencies() {
        let root = Path::new("/path/to/pi");
        if !root.exists() {
            return;
        }
        let entry = "/path/to/pi/packages/coding-agent/src/cli.ts";
        let mut texts = Vec::new();
        let handle = std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                let res = include_reachable_dependencies(root, entry, &mut texts, true);
                if let Err(e) = &res {
                    eprintln!("DEBUG_PI REACHABLE ERROR: {:?}", e);
                }
                let (resolutions, formats) = res.unwrap();
                eprintln!(
                    "DEBUG_PI REACHABLE: resolutions={}, formats={}, texts={}",
                    resolutions.len(),
                    formats.len(),
                    texts.len()
                );
                let text_map: HashMap<PathBuf, String> = texts
                    .into_iter()
                    .map(|(p, t)| (PathBuf::from(p), t))
                    .collect();

                let entries = resolutions
                    .into_iter()
                    .map(|r| {
                        (
                            PathBuf::from(r.importer),
                            r.specifier,
                            match r.condition {
                                kiln_proto::NodeRequestKind::Import => {
                                    kiln_module_resolver::NodeRequestKind::Import
                                }
                                kiln_proto::NodeRequestKind::Require => {
                                    kiln_module_resolver::NodeRequestKind::Require
                                }
                            },
                            PathBuf::from(r.target),
                        )
                    })
                    .collect::<Vec<_>>();
                let formats = formats
                    .into_iter()
                    .map(|f| {
                        (
                            PathBuf::from(f.path),
                            match f.format {
                                kiln_proto::NodeModuleFormat::EsModule => {
                                    kiln_module_resolver::NodeModuleFormat::EsModule
                                }
                                kiln_proto::NodeModuleFormat::CommonJs => {
                                    kiln_module_resolver::NodeModuleFormat::CommonJs
                                }
                            },
                        )
                    })
                    .collect::<Vec<_>>();

                let resolver = kiln_module_resolver::PinnedPackageResolver::new_with_node_builtins_requests_and_formats(
                    root,
                    entries,
                    formats,
                ).unwrap();

                let graph_res = kiln_module_graph::ModuleGraph::build_with_typed_policy(
                    Path::new(entry),
                    resolver,
                    |path, source_kind| {
                        if source_kind == kiln_module_graph::ModuleSource::Asset {
                            return Ok(kiln_module_graph::ModulePayload::AssetPath(path.to_path_buf()));
                        }
                        let text = text_map.get(path).ok_or_else(|| {
                            kiln_module_graph::ModuleGraphError::Source {
                                path: path.to_path_buf(),
                                message: format!("source is not synchronized: {}", path.display()),
                            }
                        })?;
                        if source_kind == kiln_module_graph::ModuleSource::Json {
                            let json_text = kiln_data_formats::parse_data_to_json(path, text).map_err(|err| {
                                kiln_module_graph::ModuleGraphError::Source {
                                    path: path.to_path_buf(),
                                    message: err.to_string(),
                                }
                            })?;
                            return Ok(kiln_module_graph::ModulePayload::JsonText(json_text));
                        }
                        let path_str = path.to_str().unwrap();
                        if source_kind == kiln_module_graph::ModuleSource::CommonJs {
                            let (hir, static_requires) =
                                kiln_frontend_oxc::parse_commonjs_with_requires(path_str, text);
                            if hir.has_static_module_syntax() {
                                return Err(
                                    kiln_module_graph::ModuleGraphError::CjsStaticModuleSyntax {
                                        path: path.to_path_buf(),
                                        diagnostics:
                                            kiln_ir::cjs_static_module_syntax_diagnostics(&hir),
                                    },
                                );
                            }
                            let mut program = kiln_ir::compile_commonjs(&hir).map_err(|diagnostics| {
                                kiln_module_graph::ModuleGraphError::Compile {
                                    path: path.to_path_buf(),
                                    diagnostics,
                                }
                            })?;
                            program.dependencies = static_requires;
                            return Ok(kiln_module_graph::ModulePayload::Program(program));
                        }
                        let hir = kiln_frontend_oxc::parse(path_str, text);
                        let program = kiln_ir::compile_module(&hir).map_err(|diagnostics| {
                            kiln_module_graph::ModuleGraphError::Compile {
                                path: path.to_path_buf(),
                                diagnostics,
                            }
                        })?;
                        Ok(kiln_module_graph::ModulePayload::Program(program))
                    },
                );
                eprintln!("DEBUG_PI GRAPH RESULT: {:?}", graph_res.as_ref().err());
                assert!(graph_res.is_ok());
            })
            .unwrap();
        handle.join().unwrap();
    }
}
