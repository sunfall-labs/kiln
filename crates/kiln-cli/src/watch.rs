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
use std::sync::mpsc;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, DebouncedEvent, new_debouncer};

use crate::backend::Backend;
use crate::commands;
use crate::error::{Error, Result};

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
    match commands::read_source(path) {
        Ok(current) => {
            run_once(
                backend,
                path,
                project_root,
                &target.canonical,
                current.clone(),
                out,
                notes,
            )?;
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
        let batch = incoming.recv().map_err(|_| {
            watcher_failed("the filesystem watcher stopped", "its event thread ended")
        })?;
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
                            run_once(
                                backend,
                                path,
                                project_root,
                                &target.canonical,
                                current.clone(),
                                out,
                                notes,
                            )?;
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

fn run_once(
    backend: &mut dyn Backend,
    path: &Path,
    project_root: &Path,
    canonical: &str,
    current: String,
    out: &mut dyn Write,
    notes: &mut dyn Write,
) -> Result<()> {
    // `commands::run_source` owns the warm source-sync and interpreter request path while
    // preserving the exact snapshot that the watcher compared for this save.
    let output = commands::run_source(backend, path, project_root, canonical, current)?;
    out.write_all(output.text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|source| Error::io("cannot write to stdout", source))?;
    for message in output.notes {
        note(notes, format_args!("{message}"))?;
    }
    // A failed compile/runtime execution is a normal watch iteration. Its diagnostics have
    // already been emitted above; keep the watcher alive so the next save can recover.
    Ok(())
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
    use std::io;
    use std::sync::mpsc::Sender;

    use kiln_proto::{Diagnostic, FileId, Request, Response, RunSummary, Stats};

    /// Answers like the daemon would, echoing the source as program output so tests can tell a
    /// real execution from a wake-up that found nothing to do.
    struct Fake {
        stats: Stats,
        source: String,
    }

    impl Backend for Fake {
        fn request(&mut self, request: Request) -> Result<Response> {
            Ok(match request {
                Request::Stats => Response::Stats(self.stats.clone()),
                Request::Sync { files } => Response::Needed {
                    paths: files.into_iter().map(|file| file.path).collect(),
                },
                Request::SetSource { text, .. } => {
                    self.source = text;
                    self.stats.revision += 1;
                    Response::Ok
                }
                Request::Run { path, .. } => {
                    let failed = self.source.starts_with("fail");
                    Response::Ran(RunSummary {
                        path,
                        file: FileId(1),
                        succeeded: !failed,
                        stdout: format!("program: {}\n", self.source.trim()),
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
                other => panic!("unexpected request {other:?}"),
            })
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
        let (checks, printed) = mpsc::channel();
        let (messages, notes) = mpsc::channel();
        let watched = path.to_path_buf();
        std::thread::spawn(move || {
            let mut backend = Fake {
                stats: Stats::default(),
                source: String::new(),
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
        (printed, notes)
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

        let (printed, _) = watch_in_background(&path);
        let first = printed
            .recv_timeout(Duration::from_secs(5))
            .expect("the first run should be reported");
        assert_eq!(first, "program: export const a = 1;\n");

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
}
