//! The query-pipeline commands: `check`, `project`, `stats`.
//!
//! Each one sandwiches its real work between two `Stats` reads. That delta is what makes the
//! incremental behaviour observable to a person instead of only to a test, and it is cheap:
//! two extra round trips against a check that is meant to be a millisecond.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::Instant;

use crate::backend::Backend;
use crate::error::{Error, Result};
use crate::render::{self, Work};
use crate::sources;
use kiln_proto::{NativeProfile, TestMode, TestOutcome, TestSummary};

/// What a command produced: `text` is the answer, `notes` are diagnostics for stderr.
#[derive(Debug)]
pub struct Output {
    pub text: String,
    pub notes: Vec<String>,
    pub exit_code: i32,
}

impl Output {
    pub fn new(text: String) -> Output {
        Output {
            text,
            notes: Vec::new(),
            exit_code: 0,
        }
    }
}

/// Compile and execute one file. Program stdout stays byte-for-byte on stdout; compiler and
/// runtime diagnostics go to stderr and make the command unsuccessful.
pub fn run(backend: &mut dyn Backend, path: &Path) -> Result<Output> {
    let display = path.display().to_string();
    let canonical = canonical_path(path)?;
    let text = read_source(path)?;
    run_source(backend, &display, canonical, text)
}

/// Build one source file into a native executable. Source upload is kept in the command layer so
/// daemon and one-shot backends observe exactly the same file contents.
pub fn build(
    backend: &mut dyn Backend,
    path: &Path,
    project_root: &Path,
    requested_output: Option<&Path>,
    profile: NativeProfile,
) -> Result<Output> {
    let display = path.display().to_string();
    let canonical = canonical_path(path)?;
    let text = read_source(path)?;
    backend.set_source(canonical.clone(), text)?;
    let output = native_output_path(path, project_root, requested_output)?;
    if Path::new(&output) == Path::new(&canonical)
        || std::fs::canonicalize(&output).is_ok_and(|resolved| resolved == Path::new(&canonical))
    {
        return Err(Error::BadInput(format!(
            "native output would overwrite source file {canonical}"
        )));
    }
    let summary = backend.native_build(
        canonical,
        project_root_string(project_root)?,
        Some(output),
        profile,
    )?;
    Ok(Output::new(render::native_build(&display, &summary)))
}

/// Build and execute one source file through the native backend.
pub fn native_run(
    backend: &mut dyn Backend,
    path: &Path,
    project_root: &Path,
    profile: NativeProfile,
) -> Result<Output> {
    let canonical = canonical_path(path)?;
    let text = read_source(path)?;
    backend.set_source(canonical.clone(), text)?;
    let summary = backend.native_run(canonical, project_root_string(project_root)?, profile)?;
    let mut notes = Vec::new();
    if !summary.stderr.is_empty() {
        notes.push(summary.stderr.clone());
    }
    Ok(Output {
        text: summary.stdout,
        notes,
        exit_code: summary.exit_code.unwrap_or(1),
    })
}

fn project_root_string(root: &Path) -> Result<String> {
    root.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::BadInput(format!("path is not valid UTF-8: {}", root.display())))
}

fn native_output_path(source: &Path, root: &Path, requested: Option<&Path>) -> Result<String> {
    let output = requested.map(PathBuf::from).unwrap_or_else(|| {
        let stem = source
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("module");
        let mut output = root.join(".kiln").join("bin").join(stem);
        if cfg!(windows) {
            output.set_extension("exe");
        }
        output
    });
    let output = if output.is_absolute() {
        output
    } else {
        root.join(output)
    };
    output
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::BadInput(format!("path is not valid UTF-8: {}", output.display())))
}

fn run_source(
    backend: &mut dyn Backend,
    display: &str,
    canonical: String,
    text: String,
) -> Result<Output> {
    backend.set_source(canonical.clone(), text)?;
    let summary = backend.run(canonical)?;
    let notes = summary
        .diagnostics
        .iter()
        .map(|diagnostic| match (diagnostic.start, diagnostic.end) {
            (Some(start), Some(end)) => {
                format!("{display}:{start}..{end}: {}", diagnostic.message)
            }
            _ => format!("{display}: {}", diagnostic.message),
        })
        .collect();
    Ok(Output {
        text: summary.stdout,
        notes,
        exit_code: if summary.succeeded { 0 } else { 1 },
    })
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
    let notes = summary
        .diagnostics
        .iter()
        .map(|diagnostic| match (diagnostic.start, diagnostic.end) {
            (Some(start), Some(end)) => {
                format!("{display}:{start}..{end}: {}", diagnostic.message)
            }
            _ => format!("{display}: {}", diagnostic.message),
        })
        .collect();
    Ok(Output {
        text: render::check(display, &summary, &work, after.revision, elapsed),
        notes,
        exit_code: 0,
    })
}

/// Summarise every source file under `root`.
///
/// The client owns file discovery (see [`crate::sources`]), so this pushes every file before
/// asking for the rollup. Byte-identical `SetSource` requests are no-ops; edited implementations
/// still cut off at an unchanged interface. The work report makes both kinds of reuse visible.
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
        text: render::project(&summary, &work, after.revision, elapsed),
        notes,
        exit_code: 0,
    })
}

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
) -> Result<Output> {
    let corpus = canonical_path(corpus)?;
    let state_path = project_root.join(".kiln/test-failures.json");
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

    Ok(Output {
        text: render_test(&corpus, &summary),
        notes: Vec::new(),
        exit_code: if summary.failed == 0 { 0 } else { 1 },
    })
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
    let parent = path
        .parent()
        .ok_or_else(|| Error::BadInput(format!("{} has no parent directory", path.display())))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| Error::io(format!("cannot create {}", parent.display()), error))?;
    let text = serde_json::to_string_pretty(&failures.iter().collect::<Vec<_>>())
        .map_err(|error| Error::Protocol(format!("cannot encode failure state: {error}")))?;
    let temporary = temporary_state_path(path);
    std::fs::write(&temporary, format!("{text}\n")).map_err(|error| {
        Error::io(
            format!("cannot write temporary state {}", temporary.display()),
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
    use super::*;
    use kiln_proto::{
        Diagnostic, FileId, ModuleSummary, NativeBuildSummary, NativeRunSummary, ProjectSummary,
        Request, Response, RunSummary, Stats, TestCaseSummary,
    };

    /// A backend that answers from a script and records what it was asked, so the command
    /// layer can be tested for request *order* as well as output.
    struct Fake {
        seen: Vec<Request>,
        stats: Stats,
        diagnostics: Vec<Diagnostic>,
        /// Added to the counters after each `Check`, standing in for engine work.
        per_check: Stats,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                seen: Vec::new(),
                stats: Stats::default(),
                diagnostics: Vec::new(),
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
                    Request::Run { .. } => "Run",
                    Request::NativeBuild { .. } => "NativeBuild",
                    Request::NativeRun { .. } => "NativeRun",
                    Request::Test { .. } => "Test",
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
                        diagnostics: self.diagnostics.clone(),
                    })
                }
                Request::Run { path } => Response::Ran(RunSummary {
                    path,
                    file: FileId(1),
                    succeeded: self.diagnostics.is_empty(),
                    stdout: "hello from kiln\n".to_string(),
                    value: Some("undefined".to_string()),
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
                Request::NativeRun { .. } => Response::NativeRan(NativeRunSummary {
                    exit_code: Some(0),
                    stdout: "native hello\n".to_owned(),
                    stderr: String::new(),
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
                Request::Test { .. } => Response::Tested(TestSummary::default()),
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
    fn run_preserves_program_stdout_and_reports_failures_with_a_nonzero_status() {
        let mut fake = Fake::new();
        let out = run_source(
            &mut fake,
            "main.ts",
            "/tmp/main.ts".to_string(),
            "console.log('hello from kiln')".to_string(),
        )
        .expect("run succeeds");

        assert_eq!(fake.names(), ["SetSource", "Run"]);
        assert_eq!(out.text, "hello from kiln\n");
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn native_output_defaults_to_project_bin_and_resolves_relative_paths() {
        let root = Path::new("/tmp/project");
        assert_eq!(
            native_output_path(Path::new("src/main.ts"), root, None).expect("default output"),
            root.join(".kiln")
                .join("bin")
                .join(if cfg!(windows) { "main.exe" } else { "main" })
                .to_str()
                .expect("utf-8")
        );
        assert_eq!(
            native_output_path(Path::new("src/main.ts"), root, Some(Path::new("bin/app")))
                .expect("requested output"),
            root.join("bin").join("app").to_str().expect("utf-8")
        );
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
    fn parser_diagnostics_become_user_visible_notes() {
        let mut fake = Fake::new();
        fake.diagnostics.push(Diagnostic {
            message: "expected an identifier".to_string(),
            start: Some(7),
            end: Some(8),
        });

        let out = check_source(
            &mut fake,
            "broken.ts",
            "/tmp/broken.ts".to_string(),
            "export const = 1;".to_string(),
        )
        .expect("a malformed parse is still a cached check result");

        assert!(out.text.contains("diagnostics     1"), "{}", out.text);
        assert_eq!(
            out.notes,
            ["broken.ts:7..8: expected an identifier".to_string()]
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
