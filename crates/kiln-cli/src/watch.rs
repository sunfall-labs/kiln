//! `kiln watch` - run a program initially and after every byte-changing save.
//!
//! The watcher is `notify`: FSEvents on macOS, inotify on Linux. What it replaced was a
//! 200ms poll, which spent half its interval on average before any work could begin.
//! design.md §13 budgets 200ms for a warm small edit to reach the interpreter, so a
//! placeholder that charges a quarter of that budget for nothing stops being affordable at
//! exactly the step whose purpose is making edit-to-answer small.
//!
//! Events are debounced. Editors save by writing a temporary file and renaming it over the
//! target, which arrives as several events; without a window to coalesce them one
//! keystroke-save reruns two or three times.
//!
//! Change detection still compares file *contents*, not mtimes. The two mechanisms answer
//! different questions and both are needed: notify says "something touched this path", the
//! comparison says "did the program actually change". Editors rewrite files on save whether or
//! not anything changed, and rerunning for an edit the user did not make is noise and wasted work.
//!
//! The watch is registered recursively on the project, not on one file. A watch on the file
//! follows its inode, so the first atomic save renames a new file over the target and leaves the
//! watcher staring at an unlinked one. Recursive project events also keep imported sources fresh
//! without a full filesystem walk. A target outside the project gets a second directory watch.

use std::collections::BTreeSet;
use std::fmt::Arguments;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, DebouncedEvent, new_debouncer};

use crate::backend::Backend;
use crate::backend::NativeBuildAtRevision;
use crate::commands;
use crate::error::{Error, Result};
use crate::sources;
use kiln_proto::NativeProfile;

pub fn run(
    backend: &mut dyn Backend,
    path: &Path,
    project_root: &Path,
    debounce: Duration,
    out: &mut dyn Write,
    notes: &mut dyn Write,
) -> Result<()> {
    let display = path.display().to_string();
    let target = Target::resolve(path)?;
    let project_dir = PathBuf::from(commands::canonical_path(project_root)?);

    let (events, incoming) = mpsc::channel::<DebounceEventResult>();
    let mut debouncer = new_debouncer(debounce, None, events)
        .map_err(|source| watcher_failed("cannot start the filesystem watcher", source))?;
    debouncer
        .watch(&project_dir, RecursiveMode::Recursive)
        .map_err(|source| {
            watcher_failed(format!("cannot watch {}", project_dir.display()), source)
        })?;
    if !target.dir.starts_with(&project_dir) {
        debouncer
            .watch(&target.dir, RecursiveMode::NonRecursive)
            .map_err(|source| {
                watcher_failed(format!("cannot watch {}", target.dir.display()), source)
            })?;
    }

    note(
        notes,
        format_args!(
            "watching {display} ({}ms debounce, ctrl-c to stop)",
            debounce.as_millis()
        ),
    )?;

    // The first read happens after the watcher is armed. The other order has a hole: a save
    // landing between the read and the watch is never reported and never noticed.
    let mut text: Option<String> = None;
    let mut missing = false;
    let mut project_synced = false;
    let mut source_manifest = commands::SourceManifest::new();
    let peer = match backend.peer() {
        Ok(peer) => Some(peer),
        Err(error) => {
            note(notes, format_args!("native refresh unavailable: {error}"))?;
            None
        }
    };
    let mut refresher = Refresher::new(peer);
    match commands::read_source(path) {
        Ok(current) => {
            let expectation = run_once(
                backend,
                path,
                project_root,
                SourceSnapshot {
                    canonical: &target.canonical,
                    text: current.clone(),
                },
                SourceSync::Authoritative(&mut source_manifest),
                out,
                notes,
            )?;
            project_synced = true;
            refresher.submit(
                expectation,
                target.canonical.clone(),
                project_root.to_string_lossy().into_owned(),
            );
            text = Some(current);
        }
        // Being handed a path that is not there yet is the same state the loop already has to
        // survive, entered earlier - a generated file is a normal thing to want to watch.
        // Exiting here would make `watch` the one command that cannot be started first.
        Err(err) if is_missing(&err) => {
            missing = true;
            note(notes, format_args!("{display} does not exist yet; waiting"))?;
        }
        Err(err) => return Err(err),
    }

    loop {
        let batch = match incoming.recv_timeout(debounce) {
            Ok(batch) => batch,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                refresher.drain(notes)?;
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(watcher_failed(
                    "the filesystem watcher stopped",
                    "its event thread ended",
                ));
            }
        };
        match batch {
            Ok(batch) => {
                let target_changed = batch.iter().any(|event| target.matches(event));
                let dependency_paths = batch
                    .iter()
                    .flat_map(|event| event.paths.iter())
                    .filter(|event_path| {
                        !target.matches_path(event_path)
                            && sources::is_discoverable(&project_dir, event_path)
                    })
                    .cloned()
                    .collect::<BTreeSet<_>>();
                let source_tree_removed = batch
                    .iter()
                    .flat_map(|event| event.paths.iter())
                    .filter(|event_path| !event_path.exists())
                    .any(|event_path| {
                        source_manifest
                            .keys()
                            .any(|source| Path::new(source).starts_with(event_path))
                    });
                if !target_changed && dependency_paths.is_empty() && !source_tree_removed {
                    continue;
                }

                // Existing dependencies can be updated directly from the event batch. A removal
                // needs an authoritative sync because Sync omission is the protocol's deletion
                // operation; this is deliberately the uncommon slow path.
                let mut dependency_changed = source_tree_removed;
                let mut authoritative_sync = !project_synced || source_tree_removed;
                let mut dependency_updates = Vec::new();
                for dependency in dependency_paths {
                    match commands::read_source(&dependency) {
                        Ok(source) => {
                            let canonical = commands::canonical_path(&dependency)?;
                            let hash = kiln_db::content_hash(&source);
                            if source_manifest.get(&canonical) != Some(&hash) {
                                dependency_updates.push((canonical, source, hash));
                                dependency_changed = true;
                            }
                        }
                        Err(error) if is_missing(&error) => {
                            let identity = dependency.to_string_lossy();
                            if source_manifest.contains_key(identity.as_ref()) {
                                authoritative_sync = true;
                                dependency_changed = true;
                            }
                        }
                        Err(error) => return Err(error),
                    }
                }
                if !authoritative_sync {
                    for (canonical, source, hash) in dependency_updates {
                        backend.set_source(canonical.clone(), source)?;
                        source_manifest.insert(canonical, hash);
                    }
                }
                match commands::read_source(path) {
                    Ok(current) => {
                        if missing {
                            let what = if text.is_some() {
                                "is back"
                            } else {
                                "appeared"
                            };
                            note(notes, format_args!("{display} {what}"))?;
                            missing = false;
                        }
                        if dependency_changed || text.as_deref() != Some(current.as_str()) {
                            let sync = if authoritative_sync {
                                SourceSync::Authoritative(&mut source_manifest)
                            } else {
                                SourceSync::Incremental
                            };
                            let expectation = run_once(
                                backend,
                                path,
                                project_root,
                                SourceSnapshot {
                                    canonical: &target.canonical,
                                    text: current.clone(),
                                },
                                sync,
                                out,
                                notes,
                            )?;
                            project_synced = true;
                            refresher.submit(
                                expectation,
                                target.canonical.clone(),
                                project_root.to_string_lossy().into_owned(),
                            );
                            text = Some(current);
                        }
                    }
                    // The gap in the middle of an atomic save is indistinguishable from a
                    // delete. Say so once and keep waiting; the directory watch survives
                    // either, and the last known text is kept so a recreate with the same
                    // contents does not bill the user for a recompute.
                    Err(err) if is_missing(&err) => {
                        if !missing {
                            note(notes, format_args!("{display} disappeared; still watching"))?;
                            missing = true;
                        }
                    }
                    Err(err) => return Err(err),
                }
            }
            // A backend error is not the end of the watch: the usual one is a dropped-event
            // overflow, which costs a missed rerun, not a dead command. The user is told
            // because the next `kiln run` reporting work they thought was already done
            // would otherwise be inexplicable.
            Err(errors) => {
                for error in errors {
                    note(notes, format_args!("watcher error: {error}"))?;
                }
            }
        }
        refresher.drain(notes)?;
    }
}

/// Where the watch is registered, and what the file is called inside it.
#[derive(Debug)]
struct Target {
    /// The directory handed to notify. Canonical, so it is the same directory however the
    /// user spelled the path.
    dir: PathBuf,
    /// The daemon's identity for the file. Built from the canonical directory rather than by
    /// canonicalising the file itself, so it holds still before the file exists, while it is
    /// missing mid-save, and after an editor replaces it with a different inode.
    canonical: String,
}

impl Target {
    fn resolve(path: &Path) -> Result<Target> {
        let name = path
            .file_name()
            .ok_or_else(|| Error::BadInput(format!("{} does not name a file", path.display())))?
            .to_owned();
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        // The directory is the one thing that genuinely has to exist up front, so a typo is
        // still caught here rather than becoming a watch that can never fire.
        let dir = PathBuf::from(commands::canonical_path(parent).map_err(|err| match err {
            Error::MissingFile(_) => Error::BadInput(format!(
                "cannot watch {}: the directory {} does not exist",
                path.display(),
                parent.display()
            )),
            other => other,
        })?);
        let canonical = dir.join(&name);
        let canonical = canonical
            .to_str()
            .ok_or_else(|| {
                Error::BadInput(format!("path is not valid UTF-8: {}", canonical.display()))
            })?
            .to_string();
        Ok(Target { dir, canonical })
    }

    /// Recursive project watching requires the full stable path: dependencies may share the
    /// target's file name in another directory.
    fn matches(&self, event: &DebouncedEvent) -> bool {
        event.paths.iter().any(|path| self.matches_path(path))
    }

    fn matches_path(&self, path: &Path) -> bool {
        path == Path::new(&self.canonical)
    }
}

struct SourceSnapshot<'a> {
    canonical: &'a str,
    text: String,
}

enum SourceSync<'a> {
    Authoritative(&'a mut commands::SourceManifest),
    Incremental,
}

fn run_once(
    backend: &mut dyn Backend,
    path: &Path,
    project_root: &Path,
    source: SourceSnapshot<'_>,
    sync: SourceSync<'_>,
    out: &mut dyn Write,
    notes: &mut dyn Write,
) -> Result<Option<RefreshExpectation>> {
    // Seed the daemon with one authoritative project view. Once seeded, recursive events upload
    // changed dependencies and this sends the already-read target directly; repeating the whole
    // filesystem walk on every edit would put O(project) work ahead of the incremental compiler.
    let output = match sync {
        SourceSync::Authoritative(manifest) => {
            let (output, current_manifest) = commands::run_source_with_manifest(
                backend,
                path,
                project_root,
                source.canonical,
                source.text,
            )?;
            *manifest = current_manifest;
            output
        }
        SourceSync::Incremental => commands::run_watched_source(
            backend,
            path,
            project_root,
            source.canonical,
            source.text,
        )?,
    };
    let expectation = if output.exit_code == 0 {
        output.revision.map(|revision| RefreshExpectation {
            revision,
            stdout: output.text.clone(),
        })
    } else {
        None
    };
    out.write_all(output.text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|source| Error::io("cannot write to stdout", source))?;
    for message in output.notes {
        note(notes, format_args!("{message}"))?;
    }
    // A failed compile/runtime execution is a normal watch iteration. Its diagnostics have
    // already been emitted above; keep the watcher alive so the next save can recover.
    Ok(expectation)
}

struct RefreshExpectation {
    revision: u64,
    stdout: String,
}

struct RefreshJob {
    generation: u64,
    revision: u64,
    path: String,
    project_root: String,
    expected_stdout: String,
}

struct RefreshResult {
    generation: u64,
    result: Result<NativeRefresh>,
}

struct NativeExecution {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

enum NativeRefresh {
    Executed {
        output_path: String,
        expected_stdout: String,
        execution: NativeExecution,
    },
    Stale {
        current_revision: u64,
    },
}

type NativeExecutor = Box<dyn Fn(&Path) -> Result<NativeExecution> + Send + 'static>;

#[derive(Default)]
struct RefreshQueueState {
    latest: Option<RefreshJob>,
    generation: u64,
    closed: bool,
}

#[derive(Default)]
struct RefreshQueue {
    state: Mutex<RefreshQueueState>,
    ready: Condvar,
}

impl RefreshQueue {
    fn replace(&self, job: RefreshJob) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !state.closed {
            state.generation = job.generation;
            state.latest = Some(job);
            self.ready.notify_one();
        }
    }

    fn invalidate(&self, generation: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !state.closed {
            state.generation = generation;
            state.latest = None;
        }
    }

    fn next(&self) -> Option<RefreshJob> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        loop {
            if let Some(job) = state.latest.take() {
                return Some(job);
            }
            if state.closed {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }

    fn superseded(&self, generation: u64) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.closed || state.generation > generation
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.closed = true;
        state.latest = None;
        self.ready.notify_one();
    }
}

/// A single worker and a latest-only queue. The interpreter remains on the caller thread;
/// native compilation gets its own daemon connection and cannot delay the next save.
struct Refresher {
    jobs: Option<Arc<RefreshQueue>>,
    results: mpsc::Receiver<RefreshResult>,
    generation: u64,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Refresher {
    fn new(peer: Option<Box<dyn Backend>>) -> Refresher {
        Self::new_with_executor(peer, Box::new(execute_native))
    }

    fn new_with_executor(peer: Option<Box<dyn Backend>>, executor: NativeExecutor) -> Refresher {
        let (result_tx, result_rx) = mpsc::channel::<RefreshResult>();
        if let Some(mut backend) = peer {
            let jobs = Arc::new(RefreshQueue::default());
            let worker_jobs = Arc::clone(&jobs);
            let worker = std::thread::spawn(move || {
                while let Some(job) = worker_jobs.next() {
                    let result = backend
                        .native_build_at_revision(
                            job.path,
                            job.project_root,
                            None,
                            NativeProfile::Dev,
                            job.revision,
                        )
                        .and_then(|build| match build {
                            NativeBuildAtRevision::Built(_)
                                if worker_jobs.superseded(job.generation) =>
                            {
                                Ok(NativeRefresh::Stale {
                                    current_revision: job.revision,
                                })
                            }
                            NativeBuildAtRevision::Built(summary) => {
                                let execution = executor(Path::new(&summary.output_path))?;
                                Ok(NativeRefresh::Executed {
                                    output_path: summary.output_path,
                                    expected_stdout: job.expected_stdout,
                                    execution,
                                })
                            }
                            NativeBuildAtRevision::Stale { current_revision } => {
                                Ok(NativeRefresh::Stale { current_revision })
                            }
                        });
                    if result_tx
                        .send(RefreshResult {
                            generation: job.generation,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
            Refresher {
                jobs: Some(jobs),
                results: result_rx,
                generation: 0,
                worker: Some(worker),
            }
        } else {
            Refresher {
                jobs: None,
                results: result_rx,
                generation: 0,
                worker: None,
            }
        }
    }

    fn submit(
        &mut self,
        expectation: Option<RefreshExpectation>,
        path: String,
        project_root: String,
    ) {
        self.generation += 1;
        if let Some(jobs) = &self.jobs {
            match expectation {
                Some(expectation) => jobs.replace(RefreshJob {
                    generation: self.generation,
                    revision: expectation.revision,
                    path,
                    project_root,
                    expected_stdout: expectation.stdout,
                }),
                None => jobs.invalidate(self.generation),
            }
        }
    }

    fn drain(&mut self, notes: &mut dyn Write) -> Result<()> {
        while let Ok(done) = self.results.try_recv() {
            if done.generation != self.generation {
                continue;
            }
            match done.result {
                Ok(NativeRefresh::Executed {
                    output_path,
                    expected_stdout,
                    execution,
                }) => report_native_execution(notes, &output_path, &expected_stdout, &execution)?,
                Ok(NativeRefresh::Stale { current_revision }) => {
                    let _ = current_revision;
                }
                Err(error) => note(notes, format_args!("native refresh unavailable: {error}"))?,
            }
        }
        Ok(())
    }
}

fn execute_native(path: &Path) -> Result<NativeExecution> {
    const TIMEOUT: Duration = Duration::from_secs(5);
    let mut child = Command::new(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| {
            Error::io(
                format!("cannot execute native refresh {}", path.display()),
                source,
            )
        })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Protocol("native refresh stdout pipe is unavailable".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Protocol("native refresh stderr pipe is unavailable".into()))?;
    let stdout_reader = std::thread::spawn(move || read_native_pipe(stdout));
    let stderr_reader = std::thread::spawn(move || read_native_pipe(stderr));
    let deadline = Instant::now() + TIMEOUT;
    let (status, timed_out) = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|source| Error::io("cannot wait for native refresh", source))?
        {
            break (status, false);
        }
        if Instant::now() >= deadline {
            if let Err(source) = child.kill() {
                if let Some(status) = child
                    .try_wait()
                    .map_err(|wait| Error::io("cannot wait for native refresh", wait))?
                {
                    break (status, false);
                }
                return Err(Error::io("cannot stop timed-out native refresh", source));
            }
            let status = child
                .wait()
                .map_err(|source| Error::io("cannot reap timed-out native refresh", source))?;
            break (status, true);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = join_native_pipe(stdout_reader, "stdout")?;
    let stderr = join_native_pipe(stderr_reader, "stderr")?;
    Ok(NativeExecution {
        exit_code: status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        timed_out,
    })
}

fn read_native_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_native_pipe(
    reader: std::thread::JoinHandle<std::io::Result<Vec<u8>>>,
    label: &str,
) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| Error::Protocol(format!("native refresh {label} reader panicked")))?
        .map_err(|source| Error::io(format!("cannot read native refresh {label}"), source))
}

fn report_native_execution(
    notes: &mut dyn Write,
    output_path: &str,
    expected_stdout: &str,
    execution: &NativeExecution,
) -> Result<()> {
    if !execution.timed_out && execution.exit_code == Some(0) && execution.stdout == expected_stdout
    {
        return note(
            notes,
            format_args!("native refresh stdout matched {output_path}"),
        );
    }
    let expected_stdout = debug_excerpt(expected_stdout);
    let native_stdout = debug_excerpt(&execution.stdout);
    let native_stderr = debug_excerpt(&execution.stderr);
    note(
        notes,
        format_args!(
            "native refresh mismatch {output_path}: interpreter stdout {expected_stdout}, native stdout {native_stdout}, native exit {:?}, native timeout {}, native stderr {native_stderr}",
            execution.exit_code, execution.timed_out
        ),
    )
}

fn debug_excerpt(text: &str) -> String {
    const LIMIT: usize = 512;
    if text.len() <= LIMIT {
        return format!("{text:?}");
    }
    let mut end = LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{:?}… ({} bytes)", &text[..end], text.len())
}

impl Drop for Refresher {
    fn drop(&mut self) {
        if let Some(jobs) = self.jobs.take() {
            jobs.close();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn note(notes: &mut dyn Write, message: Arguments<'_>) -> Result<()> {
    writeln!(notes, "{message}").map_err(|source| Error::io("cannot write to stderr", source))
}

/// The watcher's failures get no [`Error`] variant of their own: to a user they are all "the
/// thing that was meant to be watching your file is not".
fn watcher_failed(
    what: impl Into<String>,
    source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> Error {
    Error::io(what, std::io::Error::other(source))
}

fn is_missing(error: &Error) -> bool {
    matches!(error, Error::MissingFile(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::Sender;

    use kiln_proto::{
        Diagnostic, FileId, NativeBuildSummary, Request, Response, RunSummary, Stats,
    };

    /// Answers like the daemon would, echoing the source as program output so tests can tell a
    /// real execution from a wake-up that found nothing to do.
    struct Fake {
        stats: Stats,
        sources: HashMap<String, String>,
        requests: Option<Sender<Request>>,
    }

    impl Backend for Fake {
        fn peer(&self) -> Result<Box<dyn Backend>> {
            Ok(Box::new(NativeFake))
        }

        fn request(&mut self, request: Request) -> Result<Response> {
            if let Some(requests) = &self.requests {
                let _ = requests.send(request.clone());
            }
            Ok(match request {
                Request::Stats => Response::Stats(self.stats.clone()),
                Request::Sync { files } => Response::Needed {
                    paths: files.into_iter().map(|file| file.path).collect(),
                },
                Request::SetSource { path, text } => {
                    self.sources.insert(path, text);
                    self.stats.revision += 1;
                    Response::Ok
                }
                Request::Run { path, .. } => {
                    let source = self.sources.get(&path).cloned().unwrap_or_default();
                    let failed = source.starts_with("fail");
                    Response::Ran(RunSummary {
                        revision: self.stats.revision,
                        path,
                        file: FileId(1),
                        succeeded: !failed,
                        stdout: format!("program: {}\n", source.trim()),
                        value: Some("undefined".to_string()),
                        instructions: 1,
                        diagnostics: if failed {
                            vec![Diagnostic {
                                message: "program failed".to_string(),
                                start: None,
                                end: None,
                            }]
                        } else {
                            Vec::new()
                        },
                    })
                }
                Request::NativeBuildAtRevision { .. } => Response::Stale {
                    current_revision: self.stats.revision,
                },
                other => panic!("unexpected request {other:?}"),
            })
        }
    }

    struct NativeFake;

    impl Backend for NativeFake {
        fn request(&mut self, request: Request) -> Result<Response> {
            Ok(match request {
                Request::NativeBuildAtRevision {
                    expected_revision, ..
                } => Response::Stale {
                    current_revision: expected_revision,
                },
                other => panic!("unexpected native request {other:?}"),
            })
        }
    }

    struct ScriptedNativeBackend {
        requests: Sender<Request>,
        responses: mpsc::Receiver<Response>,
    }

    impl Backend for ScriptedNativeBackend {
        fn request(&mut self, request: Request) -> Result<Response> {
            self.requests
                .send(request)
                .map_err(|_| Error::Protocol("native request observer stopped".into()))?;
            let response = self
                .responses
                .recv()
                .map_err(|_| Error::Protocol("native response script stopped".into()))?;
            crate::backend::interpret(response)
        }
    }

    fn native_summary(output_path: &str) -> NativeBuildSummary {
        NativeBuildSummary {
            module_symbol: "kiln_module_watch".into(),
            output_path: output_path.into(),
            compiler_identity: "test-cc".into(),
            generated_c_cache_hit: false,
            app_object_cache_hit: false,
            runtime_object_cache_hit: false,
            launcher_object_cache_hit: false,
            executable_cache_hit: false,
            generated_c_bytes: 1,
            app_object_bytes: 1,
            runtime_object_bytes: 1,
            launcher_object_bytes: 1,
            executable_bytes: 1,
            emit_millis: 1,
            compile_millis: 1,
            link_millis: 1,
            total_millis: 3,
        }
    }

    /// Forwards each thing `watch` prints, so a test can wait for a re-check instead of
    /// guessing how long the watcher will take to deliver one.
    struct Tap(Sender<String>);

    impl io::Write for Tap {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let _ = self.0.send(String::from_utf8_lossy(buf).into_owned());
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct NoteTap {
        sender: Sender<String>,
        pending: String,
    }

    impl io::Write for NoteTap {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.pending.push_str(&String::from_utf8_lossy(buf));
            if self.pending.contains('\n') {
                let pending = std::mem::take(&mut self.pending);
                let _ = self.sender.send(pending);
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            if !self.pending.is_empty() {
                let _ = self.sender.send(std::mem::take(&mut self.pending));
            }
            Ok(())
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kiln-cli-watch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        dir
    }

    /// Starts a watch on `path` and hands back streams of stdout and notes.
    fn watch_in_background(path: &Path) -> (mpsc::Receiver<String>, mpsc::Receiver<String>) {
        let (printed, notes, _) = watch_in_background_observed(path);
        (printed, notes)
    }

    fn watch_in_background_observed(
        path: &Path,
    ) -> (
        mpsc::Receiver<String>,
        mpsc::Receiver<String>,
        mpsc::Receiver<Request>,
    ) {
        let (checks, printed) = mpsc::channel();
        let (messages, notes) = mpsc::channel();
        let (request_tx, requests) = mpsc::channel();
        let watched = path.to_path_buf();
        std::thread::spawn(move || {
            let mut backend = Fake {
                stats: Stats::default(),
                sources: HashMap::new(),
                requests: Some(request_tx),
            };
            let _ = run(
                &mut backend,
                &watched,
                watched.parent().expect("watch path has a parent"),
                Duration::from_millis(50),
                &mut Tap(checks),
                &mut NoteTap {
                    sender: messages,
                    pending: String::new(),
                },
            );
        });
        (printed, notes, requests)
    }

    #[test]
    fn a_file_that_is_not_there_yet_still_has_an_identity() {
        let dir = scratch("identity");
        let path = dir.join("later.js");

        let before = Target::resolve(&path).expect("resolves without the file");
        std::fs::write(&path, "export const a = 1;").expect("write");
        let after = Target::resolve(&path).expect("resolves with the file");
        assert_eq!(
            before.canonical, after.canonical,
            "a delete and recreate must not change the FileId"
        );
        assert_eq!(
            after.canonical,
            commands::canonical_path(&path).expect("canonical"),
            "watch must name the file the same way `check` does"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_directory_is_reported_not_watched() {
        let err = Target::resolve(Path::new("definitely/not/here/a.js")).expect_err("should fail");
        assert!(err.to_string().contains("does not exist"), "{err}");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn an_edit_re_checks_and_an_identical_rewrite_does_not() {
        let dir = scratch("edit");
        let path = dir.join("a.js");
        std::fs::write(&path, "export const a = 1;").expect("write");
        for index in 0..64 {
            std::fs::write(
                dir.join(format!("dependency-{index}.js")),
                format!("export const value{index} = {index};\n"),
            )
            .expect("write dependency");
        }

        let (printed, _, requests) = watch_in_background_observed(&path);
        let first = printed
            .recv_timeout(Duration::from_secs(5))
            .expect("the first run should be reported");
        assert_eq!(first, "program: export const a = 1;\n");
        let mut initial = Vec::new();
        while let Ok(request) = requests.try_recv() {
            initial.push(request);
        }
        assert_eq!(
            initial
                .iter()
                .filter(|request| matches!(request, Request::Sync { .. }))
                .count(),
            1,
            "the initial watch iteration must establish one authoritative project view"
        );
        let Request::Sync { files } = &initial[0] else {
            panic!("the initial watch request must be Sync");
        };
        assert_eq!(files.len(), 65, "the initial sync must seed every source");

        let dependency = dir.join("dependency-0.js");
        let dependency_canonical = commands::canonical_path(&dependency).expect("dependency path");
        std::fs::write(&dependency, "export const value0 = 100;\n").expect("edit dependency");
        assert_eq!(
            printed
                .recv_timeout(Duration::from_secs(5))
                .expect("a dependency edit should rerun the entry"),
            "program: export const a = 1;\n"
        );
        let mut dependency_edit = Vec::new();
        while let Ok(request) = requests.try_recv() {
            dependency_edit.push(request);
        }
        assert_eq!(
            dependency_edit.len(),
            3,
            "a dependency edit must upload the dependency and target, then run"
        );
        let Request::SetSource {
            path: uploaded_path,
            text,
        } = &dependency_edit[0]
        else {
            panic!("the changed dependency must be uploaded first");
        };
        assert_eq!(uploaded_path, &dependency_canonical);
        assert_eq!(text, "export const value0 = 100;\n");
        assert!(matches!(dependency_edit[1], Request::SetSource { .. }));
        assert!(matches!(dependency_edit[2], Request::Run { .. }));
        assert!(
            printed.recv_timeout(Duration::from_millis(250)).is_err(),
            "one dependency edit must produce one rerun"
        );

        let added = dir.join("added.js");
        std::fs::write(&added, "export const added = true;\n").expect("add dependency");
        printed
            .recv_timeout(Duration::from_secs(5))
            .expect("an added source should rerun the entry");
        let mut addition = Vec::new();
        while let Ok(request) = requests.try_recv() {
            addition.push(request);
        }
        assert_eq!(addition.len(), 3, "an addition must stay on the fast path");
        assert!(matches!(addition[0], Request::SetSource { .. }));
        assert!(matches!(addition[1], Request::SetSource { .. }));
        assert!(matches!(addition[2], Request::Run { .. }));
        assert!(
            printed.recv_timeout(Duration::from_millis(250)).is_err(),
            "one source addition must produce one rerun"
        );

        std::fs::remove_file(&dependency).expect("delete dependency");
        printed
            .recv_timeout(Duration::from_secs(5))
            .expect("a removed source should rerun the entry");
        let mut removal = Vec::new();
        while let Ok(request) = requests.try_recv() {
            removal.push(request);
        }
        assert!(
            matches!(removal.first(), Some(Request::Sync { .. })),
            "a removal must reconcile the authoritative source set: {removal:?}"
        );
        assert_eq!(
            removal
                .iter()
                .filter(|request| matches!(request, Request::Sync { .. }))
                .count(),
            1
        );
        assert!(matches!(removal.last(), Some(Request::Run { .. })));
        assert!(
            printed.recv_timeout(Duration::from_millis(250)).is_err(),
            "one source removal must produce one rerun"
        );

        // Rewriting the same bytes must not spend a recompute. That is the whole reason the
        // content comparison outlives the poll it was written for.
        std::fs::write(&path, "export const a = 1;").expect("rewrite");
        assert!(
            printed.recv_timeout(Duration::from_millis(750)).is_err(),
            "an unchanged save re-checked"
        );

        std::fs::write(&path, "export const a = 2;").expect("edit");
        let second = printed
            .recv_timeout(Duration::from_secs(5))
            .expect("a real edit should run");
        assert_eq!(second, "program: export const a = 2;\n");
        let mut warm = Vec::new();
        while let Ok(request) = requests.try_recv() {
            warm.push(request);
        }
        assert_eq!(warm.len(), 2, "a warm edit must upload and run only");
        assert!(matches!(warm[0], Request::SetSource { .. }));
        assert!(matches!(warm[1], Request::Run { .. }));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_delete_and_recreate_is_noticed_rather_than_ending_the_watch() {
        let dir = scratch("recreate");
        let path = dir.join("a.js");
        std::fs::write(&path, "export const a = 1;").expect("write");

        let (printed, _) = watch_in_background(&path);
        printed
            .recv_timeout(Duration::from_secs(5))
            .expect("the first run should be reported");

        std::fs::remove_file(&path).expect("remove");
        std::fs::write(&path, "export const a = 3;").expect("recreate");
        let after = printed
            .recv_timeout(Duration::from_secs(5))
            .expect("the recreated file should run");
        assert_eq!(after, "program: export const a = 3;\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_run_reports_diagnostics_and_recovers_on_the_next_edit() {
        let dir = scratch("failure-recovery");
        let path = dir.join("a.js");
        std::fs::write(&path, "good one").expect("write");

        let (printed, notes) = watch_in_background(&path);
        assert!(
            notes
                .recv_timeout(Duration::from_secs(5))
                .expect("watch should announce itself")
                .contains("watching")
        );
        assert_eq!(
            printed
                .recv_timeout(Duration::from_secs(5))
                .expect("the initial run should be reported"),
            "program: good one\n"
        );

        std::fs::write(&path, "fail one").expect("write failing source");
        assert_eq!(
            printed
                .recv_timeout(Duration::from_secs(5))
                .expect("the failed run should still produce program output"),
            "program: fail one\n"
        );
        let diagnostic = notes
            .recv_timeout(Duration::from_secs(5))
            .expect("the failed run should report its diagnostic");
        assert!(
            diagnostic.ends_with("a.js: program failed\n"),
            "{diagnostic}"
        );

        std::fs::write(&path, "good two").expect("write recovered source");
        assert_eq!(
            printed
                .recv_timeout(Duration::from_secs(5))
                .expect("watch should continue after a failed run"),
            "program: good two\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn native_refresh_keeps_one_build_in_flight_and_runs_only_the_latest_queued_revision() {
        let (request_tx, request_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        let backend = ScriptedNativeBackend {
            requests: request_tx,
            responses: response_rx,
        };
        let executions = Arc::new(AtomicUsize::new(0));
        let execution_count = Arc::clone(&executions);
        let mut refresher = Refresher::new_with_executor(
            Some(Box::new(backend)),
            Box::new(move |_| {
                execution_count.fetch_add(1, Ordering::SeqCst);
                Ok(NativeExecution {
                    exit_code: Some(0),
                    stdout: "revision 3\n".to_owned(),
                    stderr: String::new(),
                    timed_out: false,
                })
            }),
        );

        refresher.submit(
            Some(RefreshExpectation {
                revision: 1,
                stdout: "revision 1\n".to_owned(),
            }),
            "/project/main.js".into(),
            "/project".into(),
        );
        let first = request_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first native build starts");
        assert!(matches!(
            first,
            Request::NativeBuildAtRevision {
                expected_revision: 1,
                ..
            }
        ));

        // These arrive while revision 1 is blocked in the native toolchain. Revision 2 must
        // never start: finishing revision 1 should collapse the queue directly to revision 3.
        refresher.submit(
            Some(RefreshExpectation {
                revision: 2,
                stdout: "revision 2\n".to_owned(),
            }),
            "/project/main.js".into(),
            "/project".into(),
        );
        refresher.submit(
            Some(RefreshExpectation {
                revision: 3,
                stdout: "revision 3\n".to_owned(),
            }),
            "/project/main.js".into(),
            "/project".into(),
        );
        response_tx
            .send(Response::NativeBuilt(native_summary(
                "/project/.kiln/bin/obsolete",
            )))
            .expect("finish superseded build");

        let latest = request_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("latest queued build starts");
        assert!(matches!(
            latest,
            Request::NativeBuildAtRevision {
                expected_revision: 3,
                ..
            }
        ));
        assert!(
            request_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "the superseded middle revision must not be built"
        );
        response_tx
            .send(Response::NativeBuilt(native_summary(
                "/project/.kiln/bin/main",
            )))
            .expect("finish latest build");

        let mut notes = Vec::new();
        for _ in 0..100 {
            refresher.drain(&mut notes).expect("drain native result");
            if !notes.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let notes = String::from_utf8(notes).expect("utf-8 notes");
        assert_eq!(
            notes, "native refresh stdout matched /project/.kiln/bin/main\n",
            "the stale generation stays silent and only the latest result is reported"
        );
        assert_eq!(
            executions.load(Ordering::SeqCst),
            1,
            "only the latest revision's artifact may execute"
        );
    }

    #[test]
    fn a_failed_foreground_run_invalidates_an_older_native_artifact() {
        let (request_tx, request_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        let backend = ScriptedNativeBackend {
            requests: request_tx,
            responses: response_rx,
        };
        let executions = Arc::new(AtomicUsize::new(0));
        let execution_count = Arc::clone(&executions);
        let mut refresher = Refresher::new_with_executor(
            Some(Box::new(backend)),
            Box::new(move |_| {
                execution_count.fetch_add(1, Ordering::SeqCst);
                Ok(NativeExecution {
                    exit_code: Some(0),
                    stdout: "obsolete\n".to_owned(),
                    stderr: String::new(),
                    timed_out: false,
                })
            }),
        );

        refresher.submit(
            Some(RefreshExpectation {
                revision: 1,
                stdout: "old success\n".to_owned(),
            }),
            "/project/main.js".into(),
            "/project".into(),
        );
        request_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("native build starts");
        refresher.submit(None, "/project/main.js".into(), "/project".into());
        response_tx
            .send(Response::NativeBuilt(native_summary(
                "/project/.kiln/bin/obsolete",
            )))
            .expect("finish obsolete build");

        let done = refresher
            .results
            .recv_timeout(Duration::from_secs(1))
            .expect("obsolete result is retired");
        assert!(matches!(done.result, Ok(NativeRefresh::Stale { .. })));
        assert_eq!(executions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn native_refresh_reports_output_or_exit_mismatches_on_the_notes_stream() {
        let execution = NativeExecution {
            exit_code: Some(7),
            stdout: "native output\n".to_owned(),
            stderr: "native error\n".to_owned(),
            timed_out: false,
        };
        let mut notes = Vec::new();

        report_native_execution(
            &mut notes,
            "/project/.kiln/bin/main",
            "interpreter output\n",
            &execution,
        )
        .expect("mismatch is reported");

        let notes = String::from_utf8(notes).expect("utf-8 notes");
        assert!(notes.starts_with("native refresh mismatch /project/.kiln/bin/main:"));
        assert!(notes.contains("interpreter output\\n"), "{notes}");
        assert!(notes.contains("native output\\n"), "{notes}");
        assert!(notes.contains("native exit Some(7)"), "{notes}");
        assert!(notes.contains("native timeout false"), "{notes}");
        assert!(notes.contains("native error\\n"), "{notes}");
    }

    #[cfg(unix)]
    #[test]
    fn native_refresh_executor_captures_the_artifacts_real_process_result() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("native-executor");
        let executable = dir.join("program");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf 'native stdout\\n'\nprintf 'native stderr\\n' >&2\nexit 4\n",
        )
        .expect("write executable");
        let mut permissions = std::fs::metadata(&executable)
            .expect("executable metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).expect("make executable");

        let execution = execute_native(&executable).expect("execute fixture");
        assert_eq!(execution.exit_code, Some(4));
        assert_eq!(execution.stdout, "native stdout\n");
        assert_eq!(execution.stderr, "native stderr\n");
        assert!(!execution.timed_out);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
