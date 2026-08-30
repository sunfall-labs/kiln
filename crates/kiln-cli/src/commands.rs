//! The query-pipeline commands: `check`, `project`, `stats`.
//!
//! Each one sandwiches its real work between two `Stats` reads. That delta is what makes the
//! incremental behaviour observable to a person instead of only to a test, and it is cheap:
//! two extra round trips against a check that is meant to be a millisecond.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::Instant;

use crate::backend::{Backend, SourceResponse, SourceUpdate, SyncCommit};
use crate::error::{Error, Result};
use crate::render::{self, Work};
use crate::sources;
use kiln_proto::{
    NativeProfile, ResolutionMap, SourceDigest, SourceHash, SourceView, TestMode, TestOutcome,
    TestSummary, source_digest,
};
use serde_json::json;

/// What a command produced: `text` is the answer, `notes` are diagnostics for stderr.
#[derive(Debug)]
pub struct Output {
    pub text: String,
    pub notes: Vec<String>,
    pub exit_code: i32,
    pub revision: Option<u64>,
    /// Exact final interpreter scalar retained for watch's private native equivalence check.
    /// Commands still render only program stdout on the public stdout stream.
    pub value: Option<kiln_proto::ScalarValueSummary>,
    pub source_view: Option<SourceView>,
}

impl Output {
    pub fn new(text: String) -> Output {
        Output {
            text,
            notes: Vec::new(),
            exit_code: 0,
            revision: None,
            value: None,
            source_view: None,
        }
    }
}

/// Compile and execute one file. Program stdout stays byte-for-byte on stdout; compiler and
/// runtime diagnostics go to stderr and make the command unsuccessful.
pub fn run(backend: &mut dyn Backend, path: &Path, project_root: &Path) -> Result<Output> {
    let canonical = canonical_path(path)?;
    let text = read_source(path)?;
    run_source(backend, path, project_root, &canonical, text)
}

/// Compile and execute a source snapshot that the caller has already read. This is used by
/// `watch`: its byte comparison and the execution must refer to the same save, even if an
/// editor replaces the file again while project synchronization is walking the tree.
pub(crate) fn run_source(
    backend: &mut dyn Backend,
    path: &Path,
    project_root: &Path,
    canonical: &str,
    text: String,
) -> Result<Output> {
    run_source_with_manifest(backend, path, project_root, canonical, text)
        .map(|(output, _, _, _, _)| output)
}

pub(crate) type SourceManifest = HashMap<String, SourceDigest>;

/// Establish the authoritative source view for a watch and return the hashes derived from that
/// same single read. Subsequent filesystem events use the manifest to ignore duplicate notify
/// batches and byte-identical dependency rewrites without another project walk.
pub(crate) fn run_source_with_manifest(
    backend: &mut dyn Backend,
    path: &Path,
    project_root: &Path,
    canonical: &str,
    text: String,
) -> Result<(Output, SourceManifest, ResolutionMap, bool, SourceView)> {
    let (
        mut project_notes,
        synced_paths,
        mut manifest,
        mut resolutions,
        mut truncated,
        mut source_view,
    ) = sync_project_sources_with_override(backend, project_root, Some((canonical, &text)))?;
    // A source outside the discovered project (or an extension intentionally not walked) still
    // gets uploaded so the long-standing single-file command remains useful. In-project files
    // were already uploaded by the sync handshake when the daemon requested them.
    if !synced_paths.contains(canonical) {
        source_view = set_external_source_with_one_resync(
            backend,
            project_root,
            canonical,
            &text,
            source_view,
        )?;
    }
    let mut output = match backend.run_at_view(
        canonical.to_owned(),
        project_root_string(project_root)?,
        source_view,
    )? {
        SourceResponse::Ready(summary) => finish_run_summary(path, summary, project_notes),
        SourceResponse::Stale(_) => {
            let snapshot = establish_snapshot(backend, project_root, canonical, &text)?;
            project_notes.extend(snapshot.notes);
            let _ = snapshot.synced_paths;
            manifest = snapshot.manifest;
            resolutions = snapshot.resolutions;
            truncated = snapshot.truncated;
            source_view = snapshot.source_view;
            match backend.run_at_view(
                canonical.to_owned(),
                project_root_string(project_root)?,
                source_view,
            )? {
                SourceResponse::Ready(summary) => finish_run_summary(path, summary, project_notes),
                SourceResponse::Stale(current) => {
                    return Err(Error::Protocol(format!("stale source view {}", current.0)));
                }
            }
        }
    };
    output.source_view = Some(source_view);
    Ok((output, manifest, resolutions, truncated, source_view))
}

/// Execute a new snapshot of the single file owned by an already-initialized watch.
///
/// The first watch iteration uses [`run_source`] to establish the authoritative project view.
/// Later iterations upload changed dependencies directly from the recursive event stream, then
/// use this path for the target. Uploading the bytes already read by the watcher keeps comparison,
/// compilation, and execution on the same save.
pub(crate) fn run_watched_source_at_view(
    backend: &mut dyn Backend,
    path: &Path,
    project_root: &Path,
    canonical: &str,
    text: String,
    source_view: SourceView,
) -> std::result::Result<(Output, SourceView), ViewError> {
    let source_view = match backend
        .set_source_at_view(canonical.to_owned(), text, source_view)
        .map_err(ViewError::Other)?
    {
        SourceUpdate::Updated(view) => view,
        SourceUpdate::Stale(current) => return Err(ViewError::Stale(current)),
    };
    let summary = match backend
        .run_at_view(
            canonical.to_owned(),
            project_root_string(project_root)?,
            source_view,
        )
        .map_err(ViewError::Other)?
    {
        SourceResponse::Ready(summary) => summary,
        SourceResponse::Stale(current) => return Err(ViewError::Stale(current)),
    };
    let mut output = finish_run_summary(path, summary, Vec::new());
    output.source_view = Some(source_view);
    Ok((output, source_view))
}

#[derive(Debug)]
pub(crate) enum ViewError {
    Stale(SourceView),
    Other(Error),
}

impl From<Error> for ViewError {
    fn from(error: Error) -> Self {
        Self::Other(error)
    }
}

/// CAS an out-of-discovery source, recovering one concurrent writer by rebuilding the complete
/// authoritative view and retrying the same already-read bytes exactly once.
fn set_external_source_with_one_resync(
    backend: &mut dyn Backend,
    project_root: &Path,
    canonical: &str,
    text: &str,
    source_view: SourceView,
) -> Result<SourceView> {
    match backend.set_source_at_view(canonical.to_owned(), text.to_owned(), source_view)? {
        SourceUpdate::Updated(view) => Ok(view),
        SourceUpdate::Stale(_) => {
            let (_, _, _, _, _, view) =
                sync_project_sources_with_override(backend, project_root, Some((canonical, text)))?;
            match backend.set_source_at_view(canonical.to_owned(), text.to_owned(), view)? {
                SourceUpdate::Updated(view) => Ok(view),
                SourceUpdate::Stale(current) => Err(Error::Protocol(format!(
                    "stale source view {} after external source retry",
                    current.0
                ))),
            }
        }
    }
}

struct Snapshot {
    notes: Vec<String>,
    synced_paths: HashSet<String>,
    manifest: SourceManifest,
    resolutions: ResolutionMap,
    truncated: bool,
    source_view: SourceView,
}

fn establish_snapshot(
    backend: &mut dyn Backend,
    project_root: &Path,
    canonical: &str,
    text: &str,
) -> Result<Snapshot> {
    let (notes, paths, manifest, resolutions, truncated, view) =
        sync_project_sources_with_override(backend, project_root, Some((canonical, text)))?;
    let source_view = if paths.contains(canonical) {
        view
    } else {
        set_external_source_with_one_resync(backend, project_root, canonical, text, view)?
    };
    Ok(Snapshot {
        notes,
        synced_paths: paths,
        manifest,
        resolutions,
        truncated,
        source_view,
    })
}

fn finish_run_summary(
    path: &Path,
    summary: kiln_proto::RunSummary,
    project_notes: Vec<String>,
) -> Output {
    let display = path.display().to_string();
    let notes = summary
        .diagnostics
        .iter()
        .map(|diagnostic| match (diagnostic.start, diagnostic.end) {
            (Some(start), Some(end)) => {
                format!("{display}:{start}..{end}: {}", diagnostic.message)
            }
            _ => format!("{display}: {}", diagnostic.message),
        })
        .chain(project_notes)
        .collect();
    Output {
        text: summary.stdout,
        notes,
        exit_code: if summary.succeeded { 0 } else { 1 },
        revision: Some(summary.revision),
        value: summary.scalar_value,
        source_view: None,
    }
}

/// Build one source file into a native executable. The entry is read before project
/// synchronization and substituted into that walk, so daemon and one-shot backends observe the
/// same command-entry snapshot even when an editor saves again during synchronization.
pub fn build(
    backend: &mut dyn Backend,
    path: &Path,
    project_root: &Path,
    requested_output: Option<&Path>,
    profile: NativeProfile,
    shared: bool,
) -> Result<Output> {
    let display = path.display().to_string();
    let canonical = canonical_path(path)?;
    let text = read_source(path)?;
    let (mut project_notes, synced_paths, _, _, _, mut source_view) =
        sync_project_sources_with_override(backend, project_root, Some((&canonical, &text)))?;
    if !synced_paths.contains(&canonical) {
        source_view = set_external_source_with_one_resync(
            backend,
            project_root,
            &canonical,
            &text,
            source_view,
        )?;
    }
    let output = native_output_path(
        Path::new(&canonical),
        project_root,
        requested_output,
        shared,
    )?;
    if Path::new(&output) == Path::new(&canonical)
        || std::fs::canonicalize(&output).is_ok_and(|resolved| resolved == Path::new(&canonical))
    {
        return Err(Error::BadInput(format!(
            "native output would overwrite source file {canonical}"
        )));
    }
    let project_root_path = project_root;
    let project_root = project_root_string(project_root_path)?;
    if shared {
        let summary = match backend.native_shared_build_at_view(
            canonical.clone(),
            project_root.clone(),
            Some(output.clone()),
            profile,
            source_view,
        )? {
            SourceResponse::Ready(summary) => summary,
            SourceResponse::Stale(_) => {
                let snapshot = establish_snapshot(backend, project_root_path, &canonical, &text)?;
                project_notes.extend(snapshot.notes);
                source_view = snapshot.source_view;
                match backend.native_shared_build_at_view(
                    canonical.clone(),
                    project_root.clone(),
                    Some(output),
                    profile,
                    source_view,
                )? {
                    SourceResponse::Ready(summary) => summary,
                    SourceResponse::Stale(current) => {
                        return Err(Error::Protocol(format!("stale source view {}", current.0)));
                    }
                }
            }
        };
        return Ok(Output {
            text: render::native_shared_build(&display, &summary),
            notes: project_notes,
            exit_code: 0,
            revision: None,
            value: None,
            source_view: Some(source_view),
        });
    }
    let summary = match backend.native_build_at_view(
        canonical.clone(),
        project_root.clone(),
        Some(output.clone()),
        profile,
        source_view,
    )? {
        SourceResponse::Ready(summary) => summary,
        SourceResponse::Stale(_) => {
            let snapshot = establish_snapshot(backend, project_root_path, &canonical, &text)?;
            project_notes.extend(snapshot.notes);
            source_view = snapshot.source_view;
            match backend.native_build_at_view(
                canonical.clone(),
                project_root,
                Some(output),
                profile,
                source_view,
            )? {
                SourceResponse::Ready(summary) => summary,
                SourceResponse::Stale(current) => {
                    return Err(Error::Protocol(format!("stale source view {}", current.0)));
                }
            }
        }
    };
    Ok(Output {
        text: render::native_build(&display, &summary),
        notes: project_notes,
        exit_code: 0,
        revision: None,
        value: None,
        source_view: Some(source_view),
    })
}

/// Build and execute one source file through the native backend from one command-entry snapshot.
pub fn native_run(
    backend: &mut dyn Backend,
    path: &Path,
    project_root: &Path,
    profile: NativeProfile,
) -> Result<Output> {
    let canonical = canonical_path(path)?;
    let text = read_source(path)?;
    let (mut project_notes, synced_paths, _, _, _, mut source_view) =
        sync_project_sources_with_override(backend, project_root, Some((&canonical, &text)))?;
    if !synced_paths.contains(&canonical) {
        source_view = set_external_source_with_one_resync(
            backend,
            project_root,
            &canonical,
            &text,
            source_view,
        )?;
    }
    let project_root_text = project_root_string(project_root)?;
    let summary = match backend.native_run_at_view(
        canonical.clone(),
        project_root_text.clone(),
        profile,
        source_view,
    )? {
        SourceResponse::Ready(summary) => summary,
        SourceResponse::Stale(_) => {
            let snapshot = establish_snapshot(backend, project_root, &canonical, &text)?;
            project_notes.extend(snapshot.notes);
            source_view = snapshot.source_view;
            match backend.native_run_at_view(canonical, project_root_text, profile, source_view)? {
                SourceResponse::Ready(summary) => summary,
                SourceResponse::Stale(current) => {
                    return Err(Error::Protocol(format!("stale source view {}", current.0)));
                }
            }
        }
    };
    let mut notes = project_notes;
    if !summary.stderr.trim_end().is_empty() {
        // Trimmed because the note is rendered as a line: a program's own trailing newline would
        // otherwise print as a blank one.
        notes.push(summary.stderr.trim_end().to_owned());
    }
    Ok(Output {
        text: summary.stdout,
        notes,
        exit_code: summary.exit_code.unwrap_or(1),
        revision: None,
        value: summary.scalar_value,
        source_view: Some(source_view),
    })
}

fn project_root_string(root: &Path) -> Result<String> {
    root.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::BadInput(format!("path is not valid UTF-8: {}", root.display())))
}

fn native_output_path(
    source: &Path,
    root: &Path,
    requested: Option<&Path>,
    shared: bool,
) -> Result<String> {
    let output = if let Some(requested) = requested {
        if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            root.join(requested)
        }
    } else if shared {
        kiln_daemon::default_native_shared_output(root, source)
    } else {
        kiln_daemon::default_native_output(root, source)
    };
    output
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::BadInput(format!("path is not valid UTF-8: {}", output.display())))
}

/// Check one file: push its current contents, ask for its summary, report the cost.
pub fn check(backend: &mut dyn Backend, path: &Path, root: &Path) -> Result<Output> {
    let display = path.display().to_string();
    let canonical = canonical_path(path)?;
    let text = read_source(path)?;
    let before = backend.stats()?;
    let started = Instant::now();
    let (mut notes, synced_paths, _, _, _, mut source_view) =
        sync_project_sources_with_override(backend, root, Some((&canonical, &text)))?;
    if !synced_paths.contains(&canonical) {
        source_view =
            set_external_source_with_one_resync(backend, root, &canonical, &text, source_view)?;
    }
    let root_text = project_root_string(root)?;
    let summary = match backend.check_at_view(canonical.clone(), root_text.clone(), source_view)? {
        SourceResponse::Ready(summary) => summary,
        SourceResponse::Stale(_) => {
            let (retry_notes, retry_paths, _, _, _, view) =
                sync_project_sources_with_override(backend, root, Some((&canonical, &text)))?;
            notes.extend(retry_notes);
            source_view = view;
            if !retry_paths.contains(&canonical) {
                source_view = set_external_source_with_one_resync(
                    backend,
                    root,
                    &canonical,
                    &text,
                    source_view,
                )?;
            }
            match backend.check_at_view(canonical, root_text, source_view)? {
                SourceResponse::Ready(summary) => summary,
                SourceResponse::Stale(current) => {
                    return Err(Error::Protocol(format!("stale source view {}", current.0)));
                }
            }
        }
    };
    let elapsed = started.elapsed();
    let after = backend.stats()?;

    let work = Work::between(&before, &after);
    notes.extend(summary.diagnostics.iter().map(|diagnostic| {
        let diagnostic_path = diagnostic.path.as_deref().unwrap_or(&display);
        match (diagnostic.start, diagnostic.end) {
            (Some(start), Some(end)) => {
                format!("{diagnostic_path}:{start}..{end}: {}", diagnostic.message)
            }
            _ => format!("{diagnostic_path}: {}", diagnostic.message),
        }
    }));
    Ok(Output {
        text: render::check(&display, &summary, &work, after.revision, elapsed),
        notes,
        exit_code: if summary.diagnostics.is_empty() { 0 } else { 1 },
        revision: None,
        value: None,
        source_view: Some(source_view),
    })
}

/// Test seam for the single-source request sequence. Production [`check`] synchronizes the
/// project first so the daemon can report an entry-rooted resolved surface.
#[cfg(test)]
pub fn check_source(
    backend: &mut dyn Backend,
    display: &str,
    canonical: String,
    project_root: String,
    text: String,
) -> Result<Output> {
    let before = backend.stats()?;
    let started = Instant::now();
    let source_view = match backend.set_source_at_view(canonical.clone(), text, SourceView(0))? {
        SourceUpdate::Updated(view) => view,
        SourceUpdate::Stale(current) => {
            return Err(Error::Protocol(format!("stale source view {}", current.0)));
        }
    };
    let summary = match backend.check_at_view(canonical, project_root, source_view)? {
        SourceResponse::Ready(summary) => summary,
        SourceResponse::Stale(current) => {
            return Err(Error::Protocol(format!("stale source view {}", current.0)));
        }
    };
    let elapsed = started.elapsed();
    let after = backend.stats()?;

    let work = Work::between(&before, &after);
    let notes = summary
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let diagnostic_path = diagnostic.path.as_deref().unwrap_or(display);
            match (diagnostic.start, diagnostic.end) {
                (Some(start), Some(end)) => {
                    format!("{diagnostic_path}:{start}..{end}: {}", diagnostic.message)
                }
                _ => format!("{diagnostic_path}: {}", diagnostic.message),
            }
        })
        .collect();
    Ok(Output {
        text: render::check(display, &summary, &work, after.revision, elapsed),
        notes,
        exit_code: if summary.diagnostics.is_empty() { 0 } else { 1 },
        revision: None,
        value: None,
        source_view: None,
    })
}

/// Summarise every source file under `root`.
///
/// The client owns file discovery (see [`crate::sources`]), and a fresh process starts knowing
/// nothing about what the daemon already holds. So it names every file with its content hash
/// and pushes only the ones the daemon asks for. Sending them all was correct but wasteful: a
/// warm no-op on a 320-file project moved ~370KB of JSON to be told nothing had changed.
///
/// Byte-identical `SetSource` requests are no-ops in the engine as well, so this is an
/// optimisation of the wire rather than a correctness fix -- but it is the difference between
/// a warm command costing microseconds and costing tens of milliseconds.
pub fn project(backend: &mut dyn Backend, root: &Path) -> Result<Output> {
    let before = backend.stats()?;
    let started = Instant::now();
    let (mut notes, _, _, _, _, mut source_view) = sync_project_sources(backend, root)?;
    let summary = match backend.project_at_view(source_view)? {
        SourceResponse::Ready(summary) => summary,
        SourceResponse::Stale(_) => {
            let (retry_notes, _, _, _, _, view) = sync_project_sources(backend, root)?;
            source_view = view;
            // Preserve diagnostics from both walks; the second is normally empty.
            notes.extend(retry_notes);
            match backend.project_at_view(source_view)? {
                SourceResponse::Ready(summary) => summary,
                SourceResponse::Stale(current) => {
                    return Err(Error::Protocol(format!("stale source view {}", current.0)));
                }
            }
        }
    };
    let elapsed = started.elapsed();
    let after = backend.stats()?;

    let work = Work::between(&before, &after);
    Ok(Output {
        text: render::project(&summary, &work, after.revision, elapsed),
        notes,
        exit_code: 0,
        revision: None,
        value: None,
        source_view: Some(source_view),
    })
}

/// Synchronize all discoverable project sources, returning non-fatal walk notes. The source hash
/// and uploaded text are deliberately derived from one read of each file.
fn sync_project_sources(backend: &mut dyn Backend, root: &Path) -> SyncResult {
    sync_project_sources_with_override(backend, root, None)
}

/// Synchronize project sources, optionally substituting one already-read snapshot.
///
/// The override feeds both the advertised hash and any requested upload, so a save landing while
/// discovery runs cannot make watch execute bytes other than the ones it observed.
fn sync_project_sources_with_override(
    backend: &mut dyn Backend,
    root: &Path,
    source_override: Option<(&str, &str)>,
) -> SyncResult {
    let walk = sources::collect(root)?;
    let mut notes = Vec::new();
    if walk.truncated {
        notes.push(format!(
            "stopped after {} files; `kiln project` is meant for a project directory",
            walk.files.len()
        ));
    }
    if walk.files.is_empty() {
        notes.push(format!(
            "no JavaScript or TypeScript sources under {}",
            root.display()
        ));
    }

    let mut texts = Vec::with_capacity(walk.files.len());
    for file in &walk.files {
        let path = canonical_path(file)?;
        let text = match source_override {
            Some((override_path, override_text)) if path == override_path => {
                override_text.to_owned()
            }
            _ => read_source(file)?,
        };
        texts.push((path, text));
    }
    let resolutions = match source_override {
        Some((entry, _)) => sources::include_reachable_dependencies(root, entry, &mut texts)?,
        None => ResolutionMap::new(canonical_path(root)?, Vec::new())
            .map_err(|error| Error::Protocol(error.to_string()))?,
    };
    let manifest = texts
        .iter()
        .map(|(path, text)| (path.clone(), source_digest(text)))
        .collect::<SourceManifest>();
    let synced_paths = texts
        .iter()
        .map(|(path, _)| path.clone())
        .collect::<HashSet<_>>();
    let files = texts
        .iter()
        .map(|(path, text)| SourceHash {
            path: path.clone(),
            hash: source_digest(text),
        })
        .collect::<Vec<_>>();
    // A stale commit means another writer published while this transaction was uploading.
    // Re-run the complete begin/stage/commit sequence once using the same already-read texts;
    // never re-walk the filesystem or mix snapshots.
    for attempt in 0..=1 {
        let plan = backend.begin_sync(files.clone(), resolutions.clone())?;
        let needed = plan.paths.into_iter().collect::<HashSet<_>>();
        for (path, text) in &texts {
            if needed.contains(path) {
                backend.stage_source(plan.sync_id, path.clone(), text.clone())?;
            }
        }
        match backend.commit_sync(plan.sync_id)? {
            SyncCommit::Synced(source_view) => {
                return Ok((
                    notes,
                    synced_paths,
                    manifest,
                    resolutions,
                    walk.truncated,
                    source_view,
                ));
            }
            SyncCommit::Stale(_) if attempt == 0 => continue,
            SyncCommit::Stale(current) => {
                return Err(Error::Protocol(format!(
                    "stale source view {} after sync retry",
                    current.0
                )));
            }
        }
    }
    unreachable!()
}

type SyncResult = Result<(
    Vec<String>,
    HashSet<String>,
    SourceManifest,
    ResolutionMap,
    bool,
    SourceView,
)>;

pub fn stats(backend: &mut dyn Backend) -> Result<Output> {
    Ok(Output::new(render::stats(&backend.stats()?)))
}

/// Run an external Test262 checkout or Kiln's local smoke corpus in one resident batch.
pub fn test(
    backend: &mut dyn Backend,
    corpus: &Path,
    project_root: &Path,
    changed: bool,
    parse_only: bool,
    output: Option<&Path>,
) -> Result<Output> {
    let corpus = canonical_path(corpus)?;
    let state_path = project_root.join(".kiln/test-failures.json");
    let results_path = output.map(|output| {
        if output.is_absolute() {
            output.to_path_buf()
        } else {
            project_root.join(output)
        }
    });
    if results_path.as_ref().is_some_and(|results| {
        results == &state_path
            || std::fs::canonicalize(results)
                .ok()
                .zip(std::fs::canonicalize(&state_path).ok())
                .is_some_and(|(results, state)| results == state)
    }) {
        return Err(Error::BadInput(format!(
            "conformance results would overwrite failure state {}",
            state_path.display()
        )));
    }
    let previous = read_failures(&state_path)?;
    let changed_paths = changed
        .then(|| git_changed_paths(project_root))
        .transpose()?;
    let mode = if parse_only {
        TestMode::Parse
    } else {
        TestMode::Interpreter
    };
    let summary = backend.test(
        corpus.clone(),
        mode,
        changed_paths,
        previous.iter().cloned().collect(),
    )?;
    let failures = updated_failures(previous, &summary);
    write_failures(&state_path, &failures)?;
    if let Some(output) = results_path {
        write_conformance_results(&output, &summary)?;
    }

    Ok(Output {
        text: render_test(&corpus, &summary),
        notes: Vec::new(),
        exit_code: test_exit_code(&summary),
        revision: None,
        value: None,
        source_view: None,
    })
}

/// Exit status for `kiln test`.
///
/// A failure is a failure, and that half is obvious. The half that is not: a run which selected
/// tests and executed none of them is **not** a success, even though nothing failed. Before this,
/// `kiln test conformance/effect` exited 0 while skipping all twelve programs for want of a
/// bundle step -- a green tick certifying that nothing was checked, which is exactly what
/// `kiln_report_has_measurements` in scripts/lib.sh already refuses to let a benchmark do.
///
/// Wiring that into CI would have been worse than having no gate, because it would have looked
/// like one. A corpus that cannot run yet should say so in the exit status until it can.
///
/// Selecting nothing at all is left as success: that is an empty corpus or a filter matching
/// nothing, which the summary reports plainly and which is not a claim about the compiler.
fn test_exit_code(summary: &TestSummary) -> i32 {
    if summary.failed > 0 {
        return 1;
    }
    if summary.selected > 0 && summary.passed == 0 {
        return 1;
    }
    0
}

fn render_test(corpus: &str, summary: &TestSummary) -> String {
    let mut output = format!(
        "test {corpus}  {}ms\n  discovered      {}\n  selected        {}\n  pass            {}\n  fail            {}\n  unsupported     {}\n  skipped         {}\n",
        summary.elapsed_millis,
        summary.discovered,
        summary.selected,
        summary.passed,
        summary.failed,
        summary.unsupported,
        summary.skipped,
    );
    for case in &summary.cases {
        if matches!(case.outcome, TestOutcome::Pass) {
            continue;
        }
        let _ = writeln!(output, "  {:<15}{}", outcome_name(case.outcome), case.id);
        if let Some(diagnostic) = case.diagnostics.first() {
            let _ = writeln!(output, "    {}", diagnostic.message);
        }
    }
    // The distribution last, because on a corpus the compiler cannot execute yet it is the part
    // worth acting on: which refusals block how many tests, in the order to work them.
    if !summary.reasons.is_empty() {
        output.push_str("  reasons\n");
        for reason in &summary.reasons {
            let _ = writeln!(
                output,
                "  {:>5}  {:<15}{}",
                reason.count,
                outcome_name(reason.outcome),
                reason.reason
            );
        }
    }
    output
}

fn outcome_name(outcome: TestOutcome) -> &'static str {
    match outcome {
        TestOutcome::Pass => "pass",
        TestOutcome::Fail => "FAIL",
        TestOutcome::Unsupported => "unsupported",
        TestOutcome::Skipped => "skipped",
    }
}

fn outcome_tag(outcome: TestOutcome) -> &'static str {
    match outcome {
        TestOutcome::Pass => "pass",
        TestOutcome::Fail => "fail",
        TestOutcome::Unsupported => "unsupported",
        TestOutcome::Skipped => "skipped",
    }
}

/// Write the schema-1 document consumed by `scripts/ci-conformance.sh`.
///
/// The report is assembled from the cases rather than the summary counters so its totals always
/// agree with the detail that the gate reads. Sorting the stable ids makes repeated runs produce
/// the same document even if discovery order changes.
fn write_conformance_results(path: &Path, summary: &TestSummary) -> Result<()> {
    let mut cases = summary
        .cases
        .iter()
        .map(|case| (case.id.clone(), outcome_tag(case.outcome)))
        .collect::<Vec<_>>();
    cases.sort_by(|left, right| left.0.cmp(&right.0));
    if cases.is_empty() {
        return Err(Error::BadInput(
            "cannot write conformance results for a run that selected no cases".to_owned(),
        ));
    }
    if let Some(duplicate) = cases
        .windows(2)
        .find(|pair| pair[0].0 == pair[1].0)
        .map(|pair| pair[0].0.as_str())
    {
        return Err(Error::Protocol(format!(
            "cannot write conformance results: duplicate test id {duplicate}"
        )));
    }

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut unsupported = 0usize;
    let mut skipped = 0usize;
    let tests = cases
        .iter()
        .map(|(id, outcome)| {
            match *outcome {
                "pass" => passed += 1,
                "fail" => failed += 1,
                "unsupported" => unsupported += 1,
                "skipped" => skipped += 1,
                _ => unreachable!("outcome_tag only returns known outcomes"),
            }
            json!({"id": id, "outcome": outcome})
        })
        .collect::<Vec<_>>();
    let document = json!({
        "schema": 1,
        "runner": format!("kiln-cli {}", env!("CARGO_PKG_VERSION")),
        "totals": {
            "total": tests.len(),
            "passed": passed,
            "failed": failed,
            "unsupported": unsupported,
            "skipped": skipped,
            "not_yet_runnable": 0,
        },
        "tests": tests,
    });
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| Error::Protocol(format!("cannot encode conformance results: {error}")))?;
    write_json_atomically(path, &text, "conformance results")
}

fn read_failures(path: &Path) -> Result<BTreeSet<String>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(Error::io(format!("cannot read {}", path.display()), error)),
    };
    let failures: Vec<String> = serde_json::from_str(&text).map_err(|error| {
        Error::BadInput(format!(
            "{} is not valid Kiln failure state: {error}",
            path.display()
        ))
    })?;
    Ok(failures.into_iter().collect())
}

fn write_failures(path: &Path, failures: &BTreeSet<String>) -> Result<()> {
    let text = serde_json::to_string_pretty(&failures.iter().collect::<Vec<_>>())
        .map_err(|error| Error::Protocol(format!("cannot encode failure state: {error}")))?;
    write_json_atomically(path, &text, "failure state")
}

fn write_json_atomically(path: &Path, text: &str, label: &str) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| Error::io(format!("cannot create {}", parent.display()), error))?;
    let temporary = temporary_state_path(path);
    std::fs::write(&temporary, format!("{text}\n")).map_err(|error| {
        Error::io(
            format!("cannot write temporary {label} {}", temporary.display()),
            error,
        )
    })?;
    std::fs::rename(&temporary, path)
        .map_err(|error| Error::io(format!("cannot replace {}", path.display()), error))
}

fn temporary_state_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("test-failures.json");
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

fn updated_failures(mut failures: BTreeSet<String>, summary: &TestSummary) -> BTreeSet<String> {
    let selected_paths = summary
        .cases
        .iter()
        .map(|case| case.path.as_str())
        .collect::<BTreeSet<_>>();
    failures.retain(|id| {
        let path = id.rsplit_once('#').map_or(id.as_str(), |(path, _)| path);
        !selected_paths.contains(path)
    });
    failures.extend(
        summary
            .cases
            .iter()
            .filter(|case| matches!(case.outcome, TestOutcome::Fail))
            .map(|case| case.id.clone()),
    );
    failures
}

fn git_changed_paths(root: &Path) -> Result<Vec<String>> {
    let mut paths = BTreeSet::new();
    for arguments in [
        &["diff", "--name-only", "HEAD", "--"][..],
        &["ls-files", "--others", "--exclude-standard"][..],
    ] {
        let output = ProcessCommand::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .output()
            .map_err(|error| Error::io("cannot launch git for --changed selection", error))?;
        if !output.status.success() {
            return Err(Error::BadInput(format!(
                "cannot determine changed files under {}; `--changed` requires a Git worktree",
                root.display()
            )));
        }
        let stdout = String::from_utf8(output.stdout)
            .map_err(|_| Error::BadInput("git returned a non-UTF-8 changed path".to_owned()))?;
        paths.extend(
            stdout
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_owned),
        );
    }
    Ok(paths.into_iter().collect())
}

/// Absolute, symlink-free path, so the same file is the same [`kiln_proto::FileId`] no
/// matter which directory the user ran `kiln` from.
pub fn canonical_path(path: &Path) -> Result<String> {
    let canonical = std::fs::canonicalize(path).map_err(|source| match source.kind() {
        std::io::ErrorKind::NotFound => Error::MissingFile(path.display().to_string()),
        _ => Error::io(format!("cannot resolve {}", path.display()), source),
    })?;
    canonical
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| Error::BadInput(format!("path is not valid UTF-8: {}", canonical.display())))
}

pub fn read_source(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| match source.kind() {
        std::io::ErrorKind::InvalidData => {
            Error::BadInput(format!("{} is not valid UTF-8", path.display()))
        }
        std::io::ErrorKind::NotFound => Error::MissingFile(path.display().to_string()),
        _ => Error::io(format!("cannot read {}", path.display()), source),
    })
}

#[cfg(test)]
mod tests {

    fn summary(passed: usize, failed: usize, skipped: usize, selected: usize) -> TestSummary {
        TestSummary {
            discovered: selected,
            selected,
            passed,
            failed,
            unsupported: 0,
            skipped,
            elapsed_millis: 0,
            cases: Vec::new(),
            reasons: Vec::new(),
        }
    }

    /// The case this exists for: `kiln test conformance/effect` skipped all twelve programs for
    /// want of a bundle step and exited 0, which would have been a CI gate certifying that
    /// nothing was checked.
    #[test]
    fn a_run_that_verified_nothing_is_not_a_success() {
        assert_eq!(test_exit_code(&summary(0, 0, 12, 12)), 1, "all skipped");
        assert_eq!(test_exit_code(&summary(0, 20, 0, 20)), 1, "all failed");
        assert_eq!(
            test_exit_code(&summary(0, 8, 12, 20)),
            1,
            "failed and skipped"
        );
    }

    #[test]
    fn a_run_that_verified_something_succeeds() {
        assert_eq!(test_exit_code(&summary(2, 0, 0, 2)), 0, "everything passed");
        assert_eq!(
            test_exit_code(&summary(2, 0, 10, 12)),
            0,
            "some passed, rest skipped: a real check happened"
        );
    }

    /// Selecting nothing is not a claim about the compiler, so it is not a failure either.
    #[test]
    fn selecting_no_tests_is_not_a_failure() {
        assert_eq!(test_exit_code(&summary(0, 0, 0, 0)), 0);
    }

    use super::*;
    use kiln_proto::{
        Diagnostic, FileId, ModuleSummary, NativeBuildSummary, NativeRunSummary,
        NativeSharedLibrarySummary, ProjectSummary, Request, Response, RunSummary, Stats,
        TestCaseSummary,
    };

    /// A backend that answers from a script and records what it was asked, so the command
    /// layer can be tested for request *order* as well as output.
    struct Fake {
        seen: Vec<Request>,
        stats: Stats,
        diagnostics: Vec<Diagnostic>,
        test_summary: TestSummary,
        source_view: SourceView,
        /// Added to the counters after each `Check`, standing in for engine work.
        per_check: Stats,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                seen: Vec::new(),
                stats: Stats::default(),
                diagnostics: Vec::new(),
                test_summary: TestSummary::default(),
                per_check: Stats {
                    parse_computed: 1,
                    interface_computed: 1,
                    ..Stats::default()
                },
                source_view: SourceView(0),
            }
        }

        fn names(&self) -> Vec<&'static str> {
            self.seen
                .iter()
                .map(|request| match request {
                    Request::Ping { .. } => "Ping",
                    Request::SetSource { .. } => "SetSource",
                    Request::Sync { .. } => "Sync",
                    Request::StageSource { .. } => "StageSource",
                    Request::CommitSync { .. } => "CommitSync",
                    Request::Check { .. } => "Check",
                    Request::Run { .. } => "Run",
                    Request::NativeBuild { .. } => "NativeBuild",
                    Request::NativeBuildAtRevision { .. } => "NativeBuildAtRevision",
                    Request::NativeSharedBuild { .. } => "NativeSharedBuild",
                    Request::NativeRun { .. } => "NativeRun",
                    Request::Test { .. } => "Test",
                    Request::Project { .. } => "Project",
                    Request::Stats => "Stats",
                    Request::ResetStats => "ResetStats",
                    Request::Shutdown => "Shutdown",
                })
                .collect()
        }
    }

    impl Backend for Fake {
        fn request(&mut self, request: Request) -> Result<Response> {
            self.seen.push(request.clone());
            Ok(match request {
                Request::Stats => Response::Stats(self.stats.clone()),
                Request::SetSource { .. } => {
                    self.stats.revision += 1;
                    self.source_view.0 += 1;
                    Response::SourceUpdated {
                        source_view: self.source_view,
                    }
                }
                // A cold daemon: it holds nothing, so everything named is needed. That keeps
                // these tests asserting the push path rather than the skip path; the skip path
                // is covered end to end in the daemon's own tests.
                Request::Sync { files, .. } => Response::Needed {
                    sync_id: kiln_proto::SyncId(1),
                    paths: files.into_iter().map(|file| file.path).collect(),
                },
                Request::StageSource { .. } => Response::Ok,
                Request::CommitSync { .. } => {
                    self.source_view.0 += 1;
                    self.stats.revision += 1;
                    Response::Synced {
                        source_view: self.source_view,
                    }
                }
                Request::Check { path, .. } => {
                    self.stats.parse_computed += self.per_check.parse_computed;
                    self.stats.interface_computed += self.per_check.interface_computed;
                    self.stats.early_cutoff += self.per_check.early_cutoff;
                    self.stats.validated += self.per_check.validated;
                    Response::Checked(ModuleSummary {
                        path,
                        file: FileId(1),
                        implementation_hash: 7,
                        interface_hash: 9,
                        exports: vec!["main".to_string()],
                        resolved_exports: None,
                        diagnostics: self.diagnostics.clone(),
                    })
                }
                Request::Run { path, .. } => Response::Ran(RunSummary {
                    revision: self.stats.revision,
                    path,
                    file: FileId(1),
                    succeeded: self.diagnostics.is_empty(),
                    stdout: "hello from kiln\n".to_string(),
                    value: Some("undefined".to_string()),
                    scalar_value: self
                        .diagnostics
                        .is_empty()
                        .then_some(kiln_proto::ScalarValueSummary::Undefined),
                    instructions: 4,
                    diagnostics: self.diagnostics.clone(),
                }),
                Request::NativeBuild { output, .. } => Response::NativeBuilt(NativeBuildSummary {
                    module_symbol: "kiln_module_test".to_owned(),
                    output_path: output.unwrap_or_default(),
                    compiler_identity: "test-cc".to_owned(),
                    generated_c_cache_hit: false,
                    app_object_cache_hit: false,
                    runtime_object_cache_hit: false,
                    launcher_object_cache_hit: false,
                    executable_cache_hit: false,
                    generated_c_bytes: 1,
                    app_object_bytes: 2,
                    runtime_object_bytes: 3,
                    launcher_object_bytes: 4,
                    executable_bytes: 5,
                    emit_millis: 1,
                    compile_millis: 1,
                    link_millis: 1,
                    total_millis: 3,
                }),
                Request::NativeBuildAtRevision {
                    expected_revision, ..
                } => Response::Stale {
                    current_revision: expected_revision,
                },
                Request::NativeSharedBuild { output, .. } => {
                    let output_path = output.unwrap_or_default();
                    Response::NativeSharedBuilt(NativeSharedLibrarySummary {
                        module_symbol: "kiln_module_test".to_owned(),
                        module_api_symbol: "kiln_module_test_api_v2".to_owned(),
                        header_path: Path::new(&output_path)
                            .with_extension("h")
                            .to_string_lossy()
                            .into_owned(),
                        loader_path: Path::new(&output_path)
                            .parent()
                            .unwrap_or_else(|| Path::new("."))
                            .join(".kiln-loaders/kiln-test-loader.so")
                            .to_string_lossy()
                            .into_owned(),
                        output_path,
                        compiler_identity: "test-cc".to_owned(),
                        generated_c_cache_hit: false,
                        object_cache_hit: false,
                        library_cache_hit: false,
                        generated_c_bytes: 1,
                        object_bytes: 2,
                        library_bytes: 3,
                        emit_millis: 1,
                        compile_millis: 1,
                        link_millis: 1,
                        total_millis: 3,
                    })
                }
                Request::NativeRun { .. } => Response::NativeRan(NativeRunSummary {
                    exit_code: Some(0),
                    stdout: "native hello\n".to_owned(),
                    stderr: String::new(),
                    scalar_value: Some(kiln_proto::ScalarValueSummary::Undefined),
                    build: NativeBuildSummary {
                        module_symbol: "kiln_module_test".to_owned(),
                        output_path: "/tmp/test".to_owned(),
                        compiler_identity: "test-cc".to_owned(),
                        generated_c_cache_hit: false,
                        app_object_cache_hit: false,
                        runtime_object_cache_hit: false,
                        launcher_object_cache_hit: false,
                        executable_cache_hit: false,
                        generated_c_bytes: 1,
                        app_object_bytes: 2,
                        runtime_object_bytes: 3,
                        launcher_object_bytes: 4,
                        executable_bytes: 5,
                        emit_millis: 1,
                        compile_millis: 1,
                        link_millis: 1,
                        total_millis: 3,
                    },
                }),
                Request::Test { .. } => Response::Tested(self.test_summary.clone()),
                Request::Project { .. } => Response::Project(ProjectSummary {
                    modules: 2,
                    exports: 3,
                    interface_hash: 11,
                }),
                other => panic!("unexpected request {other:?}"),
            })
        }
    }

    struct StaleBackend {
        inner: Fake,
        stale_commits: usize,
        stale_checks: usize,
    }

    impl StaleBackend {
        fn new(stale_commits: usize, stale_checks: usize) -> Self {
            Self {
                inner: Fake::new(),
                stale_commits,
                stale_checks,
            }
        }
    }

    impl Backend for StaleBackend {
        fn request(&mut self, request: Request) -> Result<Response> {
            match request {
                Request::CommitSync { .. } if self.stale_commits > 0 => {
                    self.stale_commits -= 1;
                    self.inner.seen.push(request);
                    Ok(Response::StaleSourceView {
                        current: SourceView(99),
                    })
                }
                Request::Check { .. } if self.stale_checks > 0 => {
                    self.stale_checks -= 1;
                    self.inner.seen.push(request);
                    Ok(Response::StaleSourceView {
                        current: SourceView(99),
                    })
                }
                request => self.inner.request(request),
            }
        }
    }

    /// Simulate a save or watcher race that removes the entry after the command has sent its
    /// project snapshot but before the daemon answers the synchronization request.
    struct RemoveEntryDuringSync {
        inner: Fake,
        entry: Option<PathBuf>,
    }

    impl RemoveEntryDuringSync {
        fn new(entry: PathBuf) -> Self {
            Self {
                inner: Fake::new(),
                entry: Some(entry),
            }
        }
    }

    impl Backend for RemoveEntryDuringSync {
        fn request(&mut self, request: Request) -> Result<Response> {
            if matches!(request, Request::Sync { .. })
                && let Some(entry) = self.entry.take()
            {
                std::fs::remove_file(entry).expect("remove entry during sync");
            }
            self.inner.request(request)
        }
    }

    #[test]
    fn check_pushes_the_source_before_asking_and_brackets_it_with_stats() {
        let mut fake = Fake::new();
        let out = check_source(
            &mut fake,
            "a.js",
            "/tmp/a.js".to_string(),
            "/tmp".to_string(),
            "let x = 1;".to_string(),
        )
        .expect("check succeeds");

        assert_eq!(fake.names(), ["Stats", "SetSource", "Check", "Stats"]);
        let Request::Check {
            path, project_root, ..
        } = &fake.seen[2]
        else {
            panic!("expected rooted check request");
        };
        assert_eq!(path, "/tmp/a.js");
        assert_eq!(project_root, "/tmp");
        assert!(out.text.starts_with("a.js  r1  "), "{}", out.text);
        assert!(
            out.text.contains("recomputed      parse 1, interface 1"),
            "{}",
            out.text
        );
        assert!(out.notes.is_empty());
    }

    #[test]
    fn check_syncs_dependencies_before_requesting_a_resolved_surface() {
        let root = std::env::temp_dir().join(format!(
            "kiln-cli-check-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create check fixture");
        let dependency = root.join("dependency.js");
        let entry = root.join("entry.js");
        std::fs::write(&dependency, "export const answer = 42;\n").expect("write dependency");
        std::fs::write(&entry, "export * from './dependency.js';\n").expect("write entry");

        let mut backend = Fake::new();
        check(&mut backend, &entry, &root).expect("check graph");
        assert_eq!(
            backend.names(),
            [
                "Stats",
                "Sync",
                "StageSource",
                "StageSource",
                "CommitSync",
                "Check",
                "Stats"
            ]
        );
        let Request::Check { project_root, .. } = &backend.seen[5] else {
            panic!("expected rooted Check request");
        };
        assert_eq!(project_root, &root.to_string_lossy());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_preserves_program_stdout_and_reports_failures_with_a_nonzero_status() {
        let mut fake = Fake::new();
        let source_view = match fake
            .set_source_at_view(
                "/tmp/main.ts".to_string(),
                "console.log('hello from kiln')".to_string(),
                SourceView(0),
            )
            .expect("set source")
        {
            SourceUpdate::Updated(view) => view,
            SourceUpdate::Stale(_) => panic!("unexpected stale source"),
        };
        let summary = fake
            .run_at_view("/tmp/main.ts".to_string(), "/tmp".to_string(), source_view)
            .expect("run succeeds");
        let SourceResponse::Ready(summary) = summary else {
            panic!("unexpected stale run")
        };

        assert_eq!(fake.names(), ["SetSource", "Run"]);
        assert_eq!(summary.stdout, "hello from kiln\n");
        assert!(summary.succeeded);
    }

    #[test]
    fn run_syncs_project_sources_before_running_the_entry() {
        let root = std::env::temp_dir().join(format!("kiln-cli-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create run fixture");
        let dependency = root.join("dependency.js");
        let entry = root.join("entry.js");
        std::fs::write(&dependency, "export const answer = 42;\n").expect("write dependency");
        std::fs::write(
            &entry,
            "import { answer } from './dependency.js'; console.log(answer);\n",
        )
        .expect("write entry");

        let mut backend = Fake::new();
        let output = run(&mut backend, &entry, &root).expect("run graph");
        assert_eq!(output.text, "hello from kiln\n");
        assert_eq!(output.exit_code, 0);
        assert_eq!(
            backend.names(),
            ["Sync", "StageSource", "StageSource", "CommitSync", "Run"]
        );
        let Request::Run { project_root, .. } = backend.seen.last().expect("run request") else {
            panic!("last request must be Run");
        };
        assert_eq!(project_root, &root.to_string_lossy());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn native_output_defaults_preserve_project_relative_directories_and_explicit_paths() {
        let root = Path::new("/tmp/project");
        let executable = if cfg!(windows) {
            "main.ts.exe"
        } else {
            "main.ts"
        };
        assert_eq!(
            native_output_path(Path::new("src/main.ts"), root, None, false)
                .expect("default output"),
            root.join(".kiln")
                .join("bin")
                .join("src")
                .join(executable)
                .to_str()
                .expect("utf-8")
        );
        assert_eq!(
            native_output_path(Path::new("main.ts"), root, None, false)
                .expect("root default output"),
            root.join(".kiln")
                .join("bin")
                .join(executable)
                .to_str()
                .expect("utf-8")
        );
        assert_ne!(
            native_output_path(Path::new("src/main.ts"), root, None, false)
                .expect("source default output"),
            native_output_path(Path::new("tests/main.ts"), root, None, false)
                .expect("test default output")
        );
        assert_eq!(
            native_output_path(
                Path::new("src/main.ts"),
                root,
                Some(Path::new("bin/app")),
                false,
            )
            .expect("requested output"),
            root.join("bin").join("app").to_str().expect("utf-8")
        );
    }

    #[test]
    fn shared_native_output_defaults_to_project_lib() {
        let root = Path::new("/tmp/project");
        let expected_extension = if cfg!(target_os = "macos") {
            "dylib"
        } else if cfg!(windows) {
            "dll"
        } else {
            "so"
        };
        let expected = root
            .join(".kiln")
            .join("lib")
            .join("src")
            .join(format!("main.ts.{expected_extension}"));
        assert_eq!(
            native_output_path(Path::new("src/main.ts"), root, None, true).expect("shared output"),
            expected.to_str().expect("utf-8")
        );
    }

    #[test]
    fn shared_build_uses_dedicated_request_and_summary() {
        let root = std::env::temp_dir().join(format!("kiln-cli-shared-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create shared fixture");
        let entry = root.join("main.ts");
        std::fs::write(&entry, "export const main = 1;\n").expect("write shared fixture");

        let mut backend = Fake::new();
        let output = build(&mut backend, &entry, &root, None, NativeProfile::Dev, true)
            .expect("shared build");
        assert!(
            output.text.contains("native shared build"),
            "{}",
            output.text
        );
        assert!(output.text.contains("shared library"), "{}", output.text);
        assert!(output.text.contains("loader"), "{}", output.text);
        assert!(output.text.contains("header"), "{}", output.text);
        assert_eq!(
            backend.names(),
            ["Sync", "StageSource", "CommitSync", "NativeSharedBuild"]
        );
        let Request::NativeSharedBuild { output, .. } = backend.seen.last().expect("request")
        else {
            panic!("last request must be NativeSharedBuild");
        };
        assert!(
            output
                .as_deref()
                .is_some_and(|path| path.ends_with(".kiln/lib/main.ts.so")
                    || path.ends_with(".kiln/lib/main.ts.dylib")
                    || path.ends_with(".kiln/lib/main.ts.dll"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn native_build_uses_the_entry_snapshot_captured_before_project_sync() {
        let root = std::env::temp_dir().join(format!(
            "kiln-cli-native-build-snapshot-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create native build fixture");
        let entry = root.join("main.ts");
        let captured = "export const answer = 42;\n";
        std::fs::write(&entry, captured).expect("write native build entry");
        let canonical = canonical_path(&entry).expect("canonical native build entry");

        let mut backend = RemoveEntryDuringSync::new(entry.clone());
        build(&mut backend, &entry, &root, None, NativeProfile::Dev, false)
            .expect("native build keeps its captured entry snapshot");

        assert_eq!(
            backend.inner.names(),
            ["Sync", "StageSource", "CommitSync", "NativeBuild"]
        );
        let Request::Sync { files, .. } = &backend.inner.seen[0] else {
            panic!("first request must advertise the captured snapshot");
        };
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, canonical);
        assert_eq!(files[0].hash, source_digest(captured));
        let Request::StageSource { path, text, .. } = &backend.inner.seen[1] else {
            panic!("second request must upload the captured snapshot");
        };
        assert_eq!(path, &canonical);
        assert_eq!(text, captured);
        let Request::NativeBuild { path, .. } = &backend.inner.seen[3] else {
            panic!("third request must build the captured snapshot");
        };
        assert_eq!(path, &canonical);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn native_run_uses_the_entry_snapshot_captured_before_project_sync() {
        let root = std::env::temp_dir().join(format!(
            "kiln-cli-native-run-snapshot-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create native run fixture");
        let entry = root.join("main.ts");
        let captured = "console.log(42);\n";
        std::fs::write(&entry, captured).expect("write native run entry");
        let canonical = canonical_path(&entry).expect("canonical native run entry");

        let mut backend = RemoveEntryDuringSync::new(entry.clone());
        let output = native_run(&mut backend, &entry, &root, NativeProfile::Dev)
            .expect("native run keeps its captured entry snapshot");

        assert_eq!(output.text, "native hello\n");
        assert_eq!(
            backend.inner.names(),
            ["Sync", "StageSource", "CommitSync", "NativeRun"]
        );
        let Request::Sync { files, .. } = &backend.inner.seen[0] else {
            panic!("first request must advertise the captured snapshot");
        };
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, canonical);
        assert_eq!(files[0].hash, source_digest(captured));
        let Request::StageSource { path, text, .. } = &backend.inner.seen[1] else {
            panic!("second request must upload the captured snapshot");
        };
        assert_eq!(path, &canonical);
        assert_eq!(text, captured);
        let Request::NativeRun { path, .. } = &backend.inner.seen[3] else {
            panic!("third request must execute the captured snapshot");
        };
        assert_eq!(path, &canonical);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn conformance_results_are_sorted_and_totals_match_cases() {
        let path = std::env::temp_dir().join(format!(
            "kiln-cli-conformance-results-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let summary = TestSummary {
            // Deliberately do not use these counters: the report must describe its own cases.
            passed: 99,
            failed: 99,
            cases: vec![
                TestCaseSummary {
                    id: "z-last".to_owned(),
                    path: "z.js".to_owned(),
                    variant: "module".to_owned(),
                    outcome: TestOutcome::Skipped,
                    elapsed_nanos: 0,
                    diagnostics: Vec::new(),
                },
                TestCaseSummary {
                    id: "a-first".to_owned(),
                    path: "a.js".to_owned(),
                    variant: "module".to_owned(),
                    outcome: TestOutcome::Pass,
                    elapsed_nanos: 0,
                    diagnostics: Vec::new(),
                },
                TestCaseSummary {
                    id: "m-middle".to_owned(),
                    path: "m.js".to_owned(),
                    variant: "module".to_owned(),
                    outcome: TestOutcome::Unsupported,
                    elapsed_nanos: 0,
                    diagnostics: Vec::new(),
                },
                TestCaseSummary {
                    id: "b-fail".to_owned(),
                    path: "b.js".to_owned(),
                    variant: "module".to_owned(),
                    outcome: TestOutcome::Fail,
                    elapsed_nanos: 0,
                    diagnostics: Vec::new(),
                },
            ],
            ..TestSummary::default()
        };

        write_conformance_results(&path, &summary).expect("write results");
        let document: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read results"))
                .expect("valid results JSON");
        assert_eq!(document["schema"], 1);
        assert_eq!(
            document["runner"],
            format!("kiln-cli {}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            document["tests"],
            serde_json::json!([
                {"id": "a-first", "outcome": "pass"},
                {"id": "b-fail", "outcome": "fail"},
                {"id": "m-middle", "outcome": "unsupported"},
                {"id": "z-last", "outcome": "skipped"},
            ])
        );
        assert_eq!(
            document["totals"],
            serde_json::json!({
                "total": 4,
                "passed": 1,
                "failed": 1,
                "unsupported": 1,
                "skipped": 1,
                "not_yet_runnable": 0,
            })
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn conformance_results_refuse_empty_and_duplicate_case_sets() {
        let path = std::env::temp_dir().join(format!(
            "kiln-cli-invalid-conformance-results-{}.json",
            std::process::id()
        ));
        let empty = write_conformance_results(&path, &TestSummary::default())
            .expect_err("an empty gate document is not evidence");
        assert!(empty.to_string().contains("selected no cases"));

        let case = TestCaseSummary {
            id: "duplicate".to_owned(),
            path: "case.js".to_owned(),
            variant: "sloppy".to_owned(),
            outcome: TestOutcome::Pass,
            elapsed_nanos: 0,
            diagnostics: Vec::new(),
        };
        let duplicate = write_conformance_results(
            &path,
            &TestSummary {
                cases: vec![case.clone(), case],
                ..TestSummary::default()
            },
        )
        .expect_err("duplicate gate identities are ambiguous");
        assert!(
            duplicate
                .to_string()
                .contains("duplicate test id duplicate")
        );
        assert!(!path.exists());
    }

    #[test]
    fn conformance_results_cannot_overwrite_persisted_failure_state() {
        let root = std::env::temp_dir().join(format!(
            "kiln-cli-results-state-collision-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let corpus = root.join("corpus");
        std::fs::create_dir_all(&corpus).expect("create corpus");
        std::fs::write(corpus.join("case.js"), "let case_ = 1;").expect("write case");

        let error = test(
            &mut Fake::new(),
            &corpus,
            &root,
            false,
            false,
            Some(Path::new(".kiln/test-failures.json")),
        )
        .expect_err("results and failure state need distinct paths");
        assert!(error.to_string().contains("overwrite failure state"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn test_writes_results_before_returning_a_nonzero_test_status() {
        let root = std::env::temp_dir().join(format!("kiln-cli-test-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let corpus = root.join("corpus");
        std::fs::create_dir_all(&corpus).expect("create corpus");
        std::fs::write(corpus.join("case.js"), "let case_ = 1;").expect("write case");

        let mut fake = Fake::new();
        fake.test_summary = TestSummary {
            selected: 1,
            failed: 1,
            cases: vec![TestCaseSummary {
                id: "case.js".to_owned(),
                path: "case.js".to_owned(),
                variant: "module".to_owned(),
                outcome: TestOutcome::Fail,
                elapsed_nanos: 0,
                diagnostics: Vec::new(),
            }],
            ..TestSummary::default()
        };
        let output = test(
            &mut fake,
            &corpus,
            &root,
            false,
            false,
            Some(Path::new("target/results.json")),
        )
        .expect("test run completes");
        assert_eq!(output.exit_code, 1);
        let report = root.join("target/results.json");
        let document: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report).expect("read report"))
                .expect("valid report");
        assert_eq!(document["tests"][0]["outcome"], "fail");
        assert!(
            std::fs::read_to_string(root.join(".kiln/test-failures.json"))
                .expect("read failure state")
                .contains("case.js")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn failure_state_replaces_every_selected_variant_and_retains_unselected_failures() {
        let previous = BTreeSet::from([
            "a.js#sloppy".to_owned(),
            "a.js#strict".to_owned(),
            "untouched.js#sloppy".to_owned(),
        ]);
        let summary = TestSummary {
            cases: vec![TestCaseSummary {
                id: "a.js#strict".to_owned(),
                path: "a.js".to_owned(),
                variant: "strict".to_owned(),
                outcome: TestOutcome::Pass,
                elapsed_nanos: 1,
                diagnostics: Vec::new(),
            }],
            ..TestSummary::default()
        };

        assert_eq!(
            updated_failures(previous, &summary),
            BTreeSet::from(["untouched.js#sloppy".to_owned()])
        );
    }

    #[test]
    fn a_check_that_recomputed_nothing_says_so() {
        let mut fake = Fake::new();
        fake.per_check = Stats {
            validated: 2,
            ..Stats::default()
        };
        let out = check_source(
            &mut fake,
            "a.js",
            "/tmp/a.js".to_string(),
            "/tmp".to_string(),
            "x".to_string(),
        )
        .expect("check succeeds");
        assert!(out.text.contains("recomputed      nothing"), "{}", out.text);
        assert!(
            out.text.contains("reused          2 validated"),
            "{}",
            out.text
        );
    }

    #[test]
    fn parser_diagnostics_become_user_visible_notes() {
        let mut fake = Fake::new();
        fake.diagnostics.push(Diagnostic {
            path: None,
            message: "expected an identifier".to_string(),
            start: Some(7),
            end: Some(8),
        });

        let out = check_source(
            &mut fake,
            "broken.ts",
            "/tmp/broken.ts".to_string(),
            "/tmp".to_string(),
            "export const = 1;".to_string(),
        )
        .expect("a malformed parse is still a cached check result");

        assert!(out.text.contains("diagnostics     1"), "{}", out.text);
        assert_eq!(
            out.notes,
            ["broken.ts:7..8: expected an identifier".to_string()]
        );
        assert_eq!(out.exit_code, 1);
    }

    #[test]
    fn graph_diagnostics_make_check_unsuccessful() {
        let mut fake = Fake::new();
        fake.diagnostics.push(Diagnostic {
            path: Some("/tmp/dep.js".to_owned()),
            message: "could not resolve ./missing.js".to_owned(),
            start: None,
            end: None,
        });

        let out = check_source(
            &mut fake,
            "main.js",
            "/tmp/main.js".to_owned(),
            "/tmp".to_owned(),
            "import './missing.js';".to_owned(),
        )
        .expect("a graph failure remains a structured check result");

        assert_eq!(out.exit_code, 1);
        assert_eq!(
            out.notes,
            ["/tmp/dep.js: could not resolve ./missing.js".to_owned()]
        );
    }

    #[test]
    fn project_syncs_first_then_pushes_only_what_is_needed() {
        let dir = std::env::temp_dir().join(format!("kiln-cli-project-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("create dirs");
        std::fs::write(dir.join("src/a.js"), "export const a = 1;").expect("write");
        std::fs::write(dir.join("src/b.ts"), "export const b = 2;").expect("write");
        std::fs::write(dir.join("notes.md"), "ignore me").expect("write");

        let mut fake = Fake::new();
        let out = project(&mut fake, &dir).expect("project succeeds");

        // One Sync names every discovered file; this Fake answers as a cold daemon that holds
        // nothing, so both are then pushed. A warm daemon answers with a shorter list and the
        // SetSource requests disappear, which is the whole point of the handshake.
        assert_eq!(
            fake.names(),
            [
                "Stats",
                "Sync",
                "StageSource",
                "StageSource",
                "CommitSync",
                "Project",
                "Stats"
            ],
            "the two sources are named once, then pushed because the fake holds nothing"
        );
        let Request::Sync { files, .. } = &fake.seen[1] else {
            panic!("second request must be the sync handshake");
        };
        assert_eq!(
            files.len(),
            2,
            "notes.md is not a source and must not be named"
        );
        assert!(out.text.starts_with("project  r1  "), "{}", out.text);
        assert!(out.text.contains("modules         2"), "{}", out.text);
        assert!(out.notes.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_commit_restarts_the_complete_sync_with_the_same_texts() {
        let dir =
            std::env::temp_dir().join(format!("kiln-cli-stale-commit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture");
        let entry = dir.join("entry.js");
        std::fs::write(&entry, "export const answer = 42;\n").expect("write fixture");

        let mut backend = StaleBackend::new(1, 0);
        check(&mut backend, &entry, &dir).expect("stale commit is retried");
        let syncs = backend
            .inner
            .seen
            .iter()
            .filter(|request| matches!(request, Request::Sync { .. }))
            .count();
        let stages = backend
            .inner
            .seen
            .iter()
            .filter_map(|request| match request {
                Request::StageSource { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(syncs, 2);
        assert_eq!(
            stages,
            vec!["export const answer = 42;\n", "export const answer = 42;\n"]
        );
        assert_eq!(
            backend
                .inner
                .seen
                .iter()
                .filter(|r| matches!(r, Request::Check { .. }))
                .count(),
            1
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_check_restarts_sync_and_repeats_the_operation_once() {
        let dir = std::env::temp_dir().join(format!("kiln-cli-stale-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture");
        let entry = dir.join("entry.js");
        std::fs::write(&entry, "export const answer = 42;\n").expect("write fixture");

        let mut backend = StaleBackend::new(0, 1);
        check(&mut backend, &entry, &dir).expect("stale check is retried");
        assert_eq!(
            backend
                .inner
                .seen
                .iter()
                .filter(|r| matches!(r, Request::Sync { .. }))
                .count(),
            2
        );
        assert_eq!(
            backend
                .inner
                .seen
                .iter()
                .filter(|r| matches!(r, Request::Check { .. }))
                .count(),
            2
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_stale_commit_is_reported_without_a_third_transaction() {
        let dir =
            std::env::temp_dir().join(format!("kiln-cli-stale-bounded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture");
        let entry = dir.join("entry.js");
        std::fs::write(&entry, "export const answer = 42;\n").expect("write fixture");

        let mut backend = StaleBackend::new(2, 0);
        let error = check(&mut backend, &entry, &dir).expect_err("second stale is bounded");
        assert!(error.to_string().contains("stale source view"));
        assert_eq!(
            backend
                .inner
                .seen
                .iter()
                .filter(|r| matches!(r, Request::Sync { .. }))
                .count(),
            2
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_names_the_current_source_view_after_a_file_is_deleted() {
        let dir = std::env::temp_dir().join(format!("kiln-cli-sync-delete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create sync fixture");
        let survivor = dir.join("survivor.js");
        let deleted = dir.join("deleted.ts");
        std::fs::write(&survivor, "export const survivor = 1;\n").expect("write survivor");
        std::fs::write(&deleted, "export const deleted = 2;\n").expect("write deleted");
        let survivor_path = canonical_path(&survivor).expect("canonical survivor");
        let deleted_path = canonical_path(&deleted).expect("canonical deleted");

        let mut fake = Fake::new();
        sync_project_sources(&mut fake, &dir).expect("initial sync");
        let Request::Sync { files, .. } = &fake.seen[0] else {
            panic!("first request must be the sync handshake");
        };
        let first_paths: BTreeSet<_> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(first_paths.len(), 2);
        assert!(first_paths.contains(survivor_path.as_str()));
        assert!(first_paths.contains(deleted_path.as_str()));

        std::fs::remove_file(&deleted).expect("delete source");
        let second_start = fake.seen.len();
        sync_project_sources(&mut fake, &dir).expect("sync after deletion");
        let second_requests = &fake.seen[second_start..];
        let Request::Sync { files, .. } = &second_requests[0] else {
            panic!("first request after deletion must be the sync handshake");
        };
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, survivor_path);
        assert_eq!(
            second_requests
                .iter()
                .filter_map(|request| match request {
                    Request::StageSource { path, .. } => Some(path),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![&survivor_path]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_source_syncs_and_executes_the_already_read_snapshot() {
        let dir = std::env::temp_dir().join(format!(
            "kiln-cli-run-source-snapshot-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create snapshot fixture");
        let path = dir.join("entry.js");
        std::fs::write(&path, "newer bytes on disk\n").expect("write disk source");
        let canonical = canonical_path(&path).expect("canonical entry");
        let captured = "bytes captured by watch\n";

        let mut fake = Fake::new();
        run_source(&mut fake, &path, &dir, &canonical, captured.to_owned())
            .expect("captured snapshot runs");

        let Request::Sync { files, .. } = &fake.seen[0] else {
            panic!("first request must advertise the captured snapshot");
        };
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, canonical);
        assert_eq!(files[0].hash, source_digest(captured));
        let Request::StageSource { path, text, .. } = &fake.seen[1] else {
            panic!("second request must upload the captured snapshot");
        };
        assert_eq!(path, &canonical);
        assert_eq!(text, captured);
        let Request::Run {
            path, project_root, ..
        } = &fake.seen[3]
        else {
            panic!("third request must execute the captured snapshot");
        };
        assert_eq!(path, &canonical);
        assert_eq!(project_root, &dir.to_string_lossy());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_warm_watch_uploads_only_its_target_snapshot_before_running() {
        let path = Path::new("/tmp/project/entry.js");
        let canonical = "/tmp/project/entry.js";
        let mut fake = Fake::new();

        run_watched_source_at_view(
            &mut fake,
            path,
            Path::new("/tmp/project"),
            canonical,
            "export const answer = 43;\n".to_owned(),
            SourceView(0),
        )
        .expect("warm watch snapshot runs");

        assert_eq!(fake.names(), ["SetSource", "Run"]);
        let Request::SetSource { path, text, .. } = &fake.seen[0] else {
            panic!("warm watch must upload its target first");
        };
        assert_eq!(path, canonical);
        assert_eq!(text, "export const answer = 43;\n");
    }

    #[test]
    fn project_warns_when_there_is_nothing_to_summarise() {
        let dir = std::env::temp_dir().join(format!("kiln-cli-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");

        let mut fake = Fake::new();
        let out = project(&mut fake, &dir).expect("project succeeds");
        assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
        assert!(
            out.notes[0].contains("no JavaScript or TypeScript sources"),
            "{:?}",
            out.notes
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_a_usage_error_not_an_io_dump() {
        let mut fake = Fake::new();
        let err = check(
            &mut fake,
            Path::new("definitely/not/here.js"),
            Path::new("."),
        )
        .expect_err("should fail");
        assert!(matches!(err, Error::MissingFile(_)), "{err:?}");
        assert!(err.to_string().contains("no such file"), "{err}");
        assert!(fake.seen.is_empty(), "nothing should have been sent");
    }
}
