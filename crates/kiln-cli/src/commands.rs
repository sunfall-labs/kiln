//! The query-pipeline commands: `check`, `project`, `stats`.
//!
//! Each one sandwiches its real work between two `Stats` reads. That delta is what makes the
//! incremental behaviour observable to a person instead of only to a test, and it is cheap:
//! two extra round trips against a check that is meant to be a millisecond.

use std::path::Path;
use std::time::Instant;

use crate::backend::Backend;
use crate::error::{Error, Result};
use crate::render::{self, Work};
use crate::sources;

/// What a command produced: `text` is the answer, `notes` are diagnostics for stderr.
#[derive(Debug)]
pub struct Output {
    pub text: String,
    pub notes: Vec<String>,
}

impl Output {
    pub fn new(text: String) -> Output {
        Output {
            text,
            notes: Vec::new(),
        }
    }
}

/// Check one file: push its current contents, ask for its summary, report the cost.
pub fn check(backend: &mut dyn Backend, path: &Path) -> Result<Output> {
    let display = path.display().to_string();
    let canonical = canonical_path(path)?;
    let text = read_source(path)?;
    check_source(backend, &display, canonical, text)
}

/// The half of [`check`] that already has the file's text, so `watch` - which reads the file
/// itself to notice the change - does not have to read it a second time and risk checking a
/// different version than the one it saw.
pub fn check_source(
    backend: &mut dyn Backend,
    display: &str,
    canonical: String,
    text: String,
) -> Result<Output> {
    let before = backend.stats()?;
    let started = Instant::now();
    backend.set_source(canonical.clone(), text)?;
    let summary = backend.check(canonical)?;
    let elapsed = started.elapsed();
    let after = backend.stats()?;

    let work = Work::between(&before, &after);
    Ok(Output::new(render::check(
        display, &summary, &work, &after, elapsed,
    )))
}

/// Summarise every source file under `root`.
///
/// The client owns file discovery (see [`crate::sources`]), so this pushes every file before
/// asking for the rollup. `SetSource` bumps the revision unconditionally, so the parses are
/// redone on every invocation; what should stay flat across runs is `project computed`,
/// because unchanged interfaces cut the rollup off. That is exactly what the work report
/// shows, and it is why re-running this command is a useful thing to do.
pub fn project(backend: &mut dyn Backend, root: &Path) -> Result<Output> {
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

    let before = backend.stats()?;
    let started = Instant::now();
    for file in &walk.files {
        let text = read_source(file)?;
        backend.set_source(canonical_path(file)?, text)?;
    }
    let summary = backend.project()?;
    let elapsed = started.elapsed();
    let after = backend.stats()?;

    let work = Work::between(&before, &after);
    Ok(Output {
        text: render::project(&summary, &work, &after, elapsed),
        notes,
    })
}

pub fn stats(backend: &mut dyn Backend) -> Result<Output> {
    Ok(Output::new(render::stats(&backend.stats()?)))
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
    use super::*;
    use kiln_proto::{FileId, ModuleSummary, ProjectSummary, Request, Response, Stats};

    /// A backend that answers from a script and records what it was asked, so the command
    /// layer can be tested for request *order* as well as output.
    struct Fake {
        seen: Vec<Request>,
        stats: Stats,
        /// Added to the counters after each `Check`, standing in for engine work.
        per_check: Stats,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                seen: Vec::new(),
                stats: Stats::default(),
                per_check: Stats {
                    parse_computed: 1,
                    interface_computed: 1,
                    ..Stats::default()
                },
            }
        }

        fn names(&self) -> Vec<&'static str> {
            self.seen
                .iter()
                .map(|request| match request {
                    Request::Ping { .. } => "Ping",
                    Request::SetSource { .. } => "SetSource",
                    Request::Check { .. } => "Check",
                    Request::Project => "Project",
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
                    Response::Ok
                }
                Request::Check { path } => {
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
                    })
                }
                Request::Project => Response::Project(ProjectSummary {
                    modules: 2,
                    exports: 3,
                    interface_hash: 11,
                }),
                other => panic!("unexpected request {other:?}"),
            })
        }
    }

    #[test]
    fn check_pushes_the_source_before_asking_and_brackets_it_with_stats() {
        let mut fake = Fake::new();
        let out = check_source(
            &mut fake,
            "a.js",
            "/tmp/a.js".to_string(),
            "let x = 1;".to_string(),
        )
        .expect("check succeeds");

        assert_eq!(fake.names(), ["Stats", "SetSource", "Check", "Stats"]);
        assert!(out.text.starts_with("a.js  r1  "), "{}", out.text);
        assert!(
            out.text.contains("recomputed      parse 1, interface 1"),
            "{}",
            out.text
        );
        assert!(out.notes.is_empty());
    }

    #[test]
    fn a_check_that_recomputed_nothing_says_so() {
        let mut fake = Fake::new();
        fake.per_check = Stats {
            validated: 2,
            ..Stats::default()
        };
        let out = check_source(&mut fake, "a.js", "/tmp/a.js".to_string(), "x".to_string())
            .expect("check succeeds");
        assert!(out.text.contains("recomputed      nothing"), "{}", out.text);
        assert!(
            out.text.contains("reused          2 validated"),
            "{}",
            out.text
        );
    }

    #[test]
    fn project_pushes_every_source_it_finds_then_asks_once() {
        let dir = std::env::temp_dir().join(format!("kiln-cli-project-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("create dirs");
        std::fs::write(dir.join("src/a.js"), "export const a = 1;").expect("write");
        std::fs::write(dir.join("src/b.ts"), "export const b = 2;").expect("write");
        std::fs::write(dir.join("notes.md"), "ignore me").expect("write");

        let mut fake = Fake::new();
        let out = project(&mut fake, &dir).expect("project succeeds");

        assert_eq!(
            fake.names(),
            ["Stats", "SetSource", "SetSource", "Project", "Stats"],
            "only the two sources should be pushed"
        );
        assert!(out.text.starts_with("project  r2  "), "{}", out.text);
        assert!(out.text.contains("modules         2"), "{}", out.text);
        assert!(out.notes.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
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
        let err = check(&mut fake, Path::new("definitely/not/here.js")).expect_err("should fail");
        assert!(matches!(err, Error::MissingFile(_)), "{err:?}");
        assert!(err.to_string().contains("no such file"), "{err}");
        assert!(fake.seen.is_empty(), "nothing should have been sent");
    }
}
