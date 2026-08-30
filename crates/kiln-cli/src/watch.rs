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
//! The watch is registered on the file's *directory*, not the file. A watch on the file
//! follows its inode, so the first atomic save renames a new file over the target and leaves
//! the watcher staring at an unlinked one. The directory outlives every such swap, and it is
//! also what lets the command start on a file that does not exist yet.

use std::ffi::OsString;
use std::fmt::Arguments;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, DebouncedEvent, new_debouncer};

use crate::backend::Backend;
use crate::backend::NativeBuildAtRevision;
use crate::commands;
use crate::error::{Error, Result};
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

    let (events, incoming) = mpsc::channel::<DebounceEventResult>();
    let mut debouncer = new_debouncer(debounce, None, events)
        .map_err(|source| watcher_failed("cannot start the filesystem watcher", source))?;
    debouncer
        .watch(&target.dir, RecursiveMode::NonRecursive)
        .map_err(|source| {
            watcher_failed(format!("cannot watch {}", target.dir.display()), source)
        })?;

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
            let revision = run_once(
                backend,
                path,
                project_root,
                SourceSnapshot {
                    canonical: &target.canonical,
                    text: current.clone(),
                },
                true,
                out,
                notes,
            )?;
            project_synced = true;
            refresher.submit(
                revision,
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
                if !batch.iter().any(|event| target.matches(event)) {
                    continue;
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
                        if text.as_deref() != Some(current.as_str()) {
                            let revision = run_once(
                                backend,
                                path,
                                project_root,
                                SourceSnapshot {
                                    canonical: &target.canonical,
                                    text: current.clone(),
                                },
                                !project_synced,
                                out,
                                notes,
                            )?;
                            project_synced = true;
                            refresher.submit(
                                revision,
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
    /// File name within [`Target::dir`]; how an event is recognised as ours.
    name: OsString,
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
        Ok(Target {
            dir,
            name,
            canonical,
        })
    }

    /// Matched on the file name rather than the whole path: the watch is already scoped to
    /// one directory, and comparing full paths would depend on the backend echoing them back
    /// through the same alias the watch was registered with.
    fn matches(&self, event: &DebouncedEvent) -> bool {
        event
            .paths
            .iter()
            .any(|path| path.file_name() == Some(self.name.as_os_str()))
    }
}

struct SourceSnapshot<'a> {
    canonical: &'a str,
    text: String,
}

fn run_once(
    backend: &mut dyn Backend,
    path: &Path,
    project_root: &Path,
    source: SourceSnapshot<'_>,
    authoritative_sync: bool,
    out: &mut dyn Write,
    notes: &mut dyn Write,
) -> Result<Option<u64>> {
    // Seed the daemon with one authoritative project view. Once seeded, the watcher owns only
    // this target and sends its already-read snapshot directly; repeating the whole filesystem
    // walk on every edit would put O(project) work ahead of the incremental compiler.
    let output = if authoritative_sync {
        commands::run_source(backend, path, project_root, source.canonical, source.text)?
    } else {
        commands::run_watched_source(backend, path, project_root, source.canonical, source.text)?
    };
    out.write_all(output.text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|source| Error::io("cannot write to stdout", source))?;
    for message in output.notes {
        note(notes, format_args!("{message}"))?;
    }
    // A failed compile/runtime execution is a normal watch iteration. Its diagnostics have
    // already been emitted above; keep the watcher alive so the next save can recover.
    Ok(output.revision)
}

struct RefreshJob {
    generation: u64,
    revision: u64,
    path: String,
    project_root: String,
}

struct RefreshResult {
    generation: u64,
    result: Result<NativeBuildAtRevision>,
}

#[derive(Default)]
struct RefreshQueueState {
    latest: Option<RefreshJob>,
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
            state.latest = Some(job);
            self.ready.notify_one();
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
        let (result_tx, result_rx) = mpsc::channel::<RefreshResult>();
        if let Some(mut backend) = peer {
            let jobs = Arc::new(RefreshQueue::default());
            let worker_jobs = Arc::clone(&jobs);
            let worker = std::thread::spawn(move || {
                while let Some(job) = worker_jobs.next() {
                    let result = backend.native_build_at_revision(
                        job.path,
                        job.project_root,
                        None,
                        NativeProfile::Dev,
                        job.revision,
                    );
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

    fn submit(&mut self, revision: Option<u64>, path: String, project_root: String) {
        let Some(revision) = revision else { return };
        self.generation += 1;
        if let Some(jobs) = &self.jobs {
            jobs.replace(RefreshJob {
                generation: self.generation,
                revision,
                path,
                project_root,
            });
        }
    }

    fn drain(&mut self, notes: &mut dyn Write) -> Result<()> {
        while let Ok(done) = self.results.try_recv() {
            if done.generation != self.generation {
                continue;
            }
            match done.result {
                Ok(NativeBuildAtRevision::Built(summary)) => note(
                    notes,
                    format_args!("native refresh built {}", summary.output_path),
                )?,
                Ok(NativeBuildAtRevision::Stale { current_revision }) => {
                    let _ = current_revision;
                }
                Err(error) => note(notes, format_args!("native refresh unavailable: {error}"))?,
            }
        }
        Ok(())
    }
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
        assert_eq!(before.name, OsString::from("later.js"));

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
        let mut refresher = Refresher::new(Some(Box::new(backend)));

        refresher.submit(Some(1), "/project/main.js".into(), "/project".into());
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
        refresher.submit(Some(2), "/project/main.js".into(), "/project".into());
        refresher.submit(Some(3), "/project/main.js".into(), "/project".into());
        response_tx
            .send(Response::Stale {
                current_revision: 3,
            })
            .expect("finish stale build");

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
            notes, "native refresh built /project/.kiln/bin/main\n",
            "the stale generation stays silent and only the latest result is reported"
        );
    }
}
