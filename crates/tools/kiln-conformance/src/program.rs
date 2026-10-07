//! Running a program file against the stdout a corpus recorded for it.
//!
//! The contract is deliberately narrow, because it is the one the manifest can state: the program
//! runs to completion and writes exactly the recorded bytes, or it does not.  Everything else is a
//! failure that keeps its reason — a lowering refusal, a runtime error, or a diff — because those
//! reasons are the interpreter backlog and collapsing them to a boolean throws the backlog away.

use std::fs;
use std::path::Path;
use std::time::Instant;

use kiln_frontend_oxc::parse_with_goal;
use kiln_hir::SourceGoal;
use kiln_interpreter::{
    ExecutionLimits, execute_node_with_clock_and_strictness, execute_with_strictness,
};
use kiln_module_eval::ModuleGraphExecution;
use kiln_module_graph::ModuleGraph;
use kiln_module_resolver::{
    NodeModuleFormat, NodeRuntimeResolver, ResolutionKind, ResolutionPolicy, ResolvedModule,
};
use kiln_runtime::CaptureHost;

use crate::case::{
    CaseResult, Diagnostic, DiagnosticPhase, Outcome, RunMode, Variant, VariantResult,
};
use crate::worker::InterpreterWorker;

/// How much of either side of a stdout diff a one-line report shows before it elides.
const DIFF_EXCERPT_BYTES: usize = 160;

/// Run `path` and judge its output against `expected`.
///
/// The single reported variant is [`Variant::Sloppy`]: a recorded stdout is an observation of one
/// ordinary script run, and Test262's strict-directive expansion would be judging a program the
/// corpus never observed.
pub(crate) fn run_against_expected_stdout_with_worker(
    path: &Path,
    expected: Expectation<'_>,
    mode: RunMode,
    worker: Option<&mut InterpreterWorker>,
) -> VariantResult {
    VariantResult {
        variant: Variant::Sloppy,
        result: run_with_executor(path, expected, mode, false, worker),
    }
}

/// Run a source file through the same compiler but with the Tokio-owned Node Adapter installed.
pub fn run_node_against_expected_stdout(
    path: &Path,
    expected: &str,
    mode: RunMode,
) -> VariantResult {
    run_node_against_expectation(path, Expectation::stdout(expected), mode)
}

/// What a corpus program must do: write exactly `stdout`, and, when `uncaught` is set, end with
/// an uncaught exception whose report contains it -- how node ends a program with exit status 1,
/// which a recorded stdout alone cannot say.
#[derive(Debug, Clone, Copy)]
pub struct Expectation<'a> {
    pub stdout: &'a str,
    pub uncaught: Option<&'a str>,
}

impl<'a> Expectation<'a> {
    /// A program that runs to completion and writes exactly `stdout`.
    pub fn stdout(stdout: &'a str) -> Self {
        Self {
            stdout,
            uncaught: None,
        }
    }
}

/// [`run_node_against_expected_stdout`] for a program that may be expected to fail.
pub fn run_node_against_expectation(
    path: &Path,
    expected: Expectation<'_>,
    mode: RunMode,
) -> VariantResult {
    run_node_against_expectation_with_clock(
        path,
        expected,
        mode,
        kiln_runtime::ClockCapability::Virtual,
    )
}

/// [`run_node_against_expectation`] with a manifest-selected Node clock policy.
pub fn run_node_against_expectation_with_clock(
    path: &Path,
    expected: Expectation<'_>,
    mode: RunMode,
    clock: kiln_runtime::ClockCapability,
) -> VariantResult {
    VariantResult {
        variant: Variant::Sloppy,
        result: if is_javascript_source(path) {
            run_node_module(path, expected, mode, clock)
        } else {
            run_with_executor_with_clock(path, expected, mode, true, None, clock)
        },
    }
}

/// Run a Node module through the production resolver, graph, linker, shared realm session, and
/// Tokio-owned Adapter. Keeping this path in the corpus prevents a single-file test executor from
/// becoming a second, easier-to-pass Node runtime that real programs never use.
fn run_node_module(
    path: &Path,
    expected: Expectation<'_>,
    mode: RunMode,
    clock: kiln_runtime::ClockCapability,
) -> CaseResult {
    let started = Instant::now();
    if let Some(reason) = uninstalled_packages_requirement(path) {
        return skipped(&reason, started);
    }

    let project_root = path.parent().unwrap_or_else(|| Path::new("."));
    let resolver = match NodeRuntimeResolver::new(project_root) {
        Ok(resolver) => resolver,
        Err(error) => return failed(DiagnosticPhase::Harness, error.to_string(), started),
    };
    let resolved_entry = ResolvedModule::new(path.to_path_buf(), ResolutionKind::Relative);
    let format = match resolver.node_format(&resolved_entry) {
        Ok(Some(format)) => format,
        Ok(None) => {
            return failed(
                DiagnosticPhase::Harness,
                format!("Node resolver did not classify source {}", path.display()),
                started,
            );
        }
        Err(error) => return failed(DiagnosticPhase::Harness, error.to_string(), started),
    };

    if matches!(mode, RunMode::Parse) {
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) => {
                return failed(
                    DiagnosticPhase::Harness,
                    format!("cannot read {}: {error}", path.display()),
                    started,
                );
            }
        };
        let module = parse_with_goal(
            &path.to_string_lossy(),
            &source,
            source_goal_for_node_format(format),
        );
        return if module.diagnostics.is_empty() && !module.panicked {
            CaseResult::new(Outcome::Pass, Vec::new(), String::new(), started)
        } else {
            failed(
                DiagnosticPhase::Parse,
                module
                    .diagnostics
                    .first()
                    .map_or("frontend parser could not recover a module", |diagnostic| {
                        diagnostic.message.as_str()
                    }),
                started,
            )
        };
    }

    struct NodeFrontend;
    impl kiln_module_graph::Frontend for NodeFrontend {
        fn parse(&mut self, path: &str, source: &str) -> kiln_hir::Module {
            parse_with_goal(path, source, SourceGoal::Module)
        }

        fn parse_commonjs(&mut self, path: &str, source: &str) -> kiln_module_graph::CommonJsParse {
            let (module, static_requires) =
                kiln_frontend_oxc::parse_commonjs_with_requires(path, source);
            kiln_module_graph::CommonJsParse {
                module,
                static_requires,
            }
        }
    }
    let graph = match ModuleGraph::build_node(
        path,
        resolver,
        |module_path: &Path| fs::read_to_string(module_path),
        NodeFrontend,
    ) {
        Ok(graph) => graph,
        Err(error) => return failed(DiagnosticPhase::Compile, error.to_string(), started),
    };

    // A test is a program of its own: a signal it sends itself stays in it, never reaching a
    // daemon or a runner that hosts it (realms issue 26).
    let _hosted = kiln_runtime::signals::host_program();
    let mut host = CaptureHost::new();
    let outcome = graph
        .execute_node_with_clock(
            &mut host,
            ExecutionLimits::default(),
            crate::source_compiler::capabilities(path.parent()),
            clock,
        )
        .map(|_| ())
        .map_err(|error| error.to_string());
    judge(outcome, host.into_stdout(), expected, started)
}

fn run_with_executor(
    path: &Path,
    expected: Expectation<'_>,
    mode: RunMode,
    node: bool,
    worker: Option<&mut InterpreterWorker>,
) -> CaseResult {
    run_with_executor_with_clock(
        path,
        expected,
        mode,
        node,
        worker,
        kiln_runtime::ClockCapability::Virtual,
    )
}

fn run_with_executor_with_clock(
    path: &Path,
    expected: Expectation<'_>,
    mode: RunMode,
    node: bool,
    worker: Option<&mut InterpreterWorker>,
    clock: kiln_runtime::ClockCapability,
) -> CaseResult {
    let started = Instant::now();
    if let Some(reason) = out_of_process_build_requirement(path) {
        return skipped(reason, started);
    }
    if let Some(reason) = uninstalled_packages_requirement(path) {
        return skipped(&reason, started);
    }

    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        // A manifest entry with no file is a corpus defect, not a missing capability, so it fails
        // rather than skipping: a corpus may not lose a test by having its path go stale.
        Err(error) => {
            return failed(
                DiagnosticPhase::Harness,
                format!("cannot read {}: {error}", path.display()),
                started,
            );
        }
    };

    let display_path = path.to_string_lossy();
    let module = parse_with_goal(display_path.as_ref(), &source, source_goal(path));
    if !module.diagnostics.is_empty() {
        return failed_with(
            module
                .diagnostics
                .iter()
                .map(|diagnostic| {
                    Diagnostic::new(
                        DiagnosticPhase::Parse,
                        diagnostic.message.clone(),
                        diagnostic.span,
                    )
                })
                .collect(),
            started,
        );
    }
    if module.panicked {
        return failed(
            DiagnosticPhase::Parse,
            "frontend parser could not recover a program",
            started,
        );
    }
    if matches!(mode, RunMode::Parse) {
        return CaseResult::new(Outcome::Pass, Vec::new(), String::new(), started);
    }

    // Compiled *and* asked which bodies came out strict. The two halves have to travel together:
    // the goal decides strictness, and handing the machine `Strictness::sloppy` for a program the
    // compiler compiled strict makes `this` in a bare call inside an `.mjs` the global object.
    let (program, strictness) =
        match kiln_ir::compile_with_strictness(&module, kiln_ir::CompileOptions::default()) {
            Ok(compiled) => compiled,
            Err(diagnostics) if diagnostics.is_empty() => {
                return failed(
                    DiagnosticPhase::Compile,
                    "lowering refused the program without saying why",
                    started,
                );
            }
            Err(diagnostics) => {
                return failed_with(
                    diagnostics
                        .into_iter()
                        .map(|diagnostic| {
                            Diagnostic::new(
                                DiagnosticPhase::Compile,
                                diagnostic.message,
                                diagnostic.span,
                            )
                        })
                        .collect(),
                    started,
                );
            }
        };

    let _hosted = kiln_runtime::signals::host_program();
    let mut host = CaptureHost::new();
    let program = std::rc::Rc::new(program);
    let strictness = std::rc::Rc::new(strictness);
    let result = if node {
        execute_node_with_clock_and_strictness(
            program,
            strictness,
            &mut host,
            ExecutionLimits::default(),
            crate::source_compiler::capabilities(path.parent())
                .with_realm_bootstrap(kiln_module_eval::realm_bootstrap(true)),
            clock,
        )
        .map(|_| ())
    } else if let Some(worker) = worker {
        worker
            .execute_reclaiming(
                program,
                strictness,
                &mut host,
                ExecutionLimits::default(),
                crate::source_compiler::capabilities(path.parent()),
            )
            .map(|_| ())
    } else {
        execute_with_strictness(
            program,
            strictness,
            &mut host,
            ExecutionLimits::default(),
            crate::source_compiler::capabilities(path.parent()),
        )
        .map(|_| ())
    };
    judge(
        result.map_err(|error| error.to_string()),
        host.into_stdout(),
        expected,
        started,
    )
}

/// How the run ended against how it was expected to: an uncaught exception is a failure unless
/// the corpus recorded that one, as node's exit 1, and then the stdout before it is still judged.
fn judge(
    outcome: Result<(), String>,
    stdout: String,
    expected: Expectation<'_>,
    started: Instant,
) -> CaseResult {
    match (outcome, expected.uncaught) {
        (Err(error), Some(uncaught)) if error.contains(uncaught) => {}
        (Err(error), _) => {
            let mut result = failed(DiagnosticPhase::Runtime, error, started);
            result.stdout = stdout;
            return result;
        }
        (Ok(()), Some(uncaught)) => {
            let mut result = failed(
                DiagnosticPhase::Expectation,
                format!(
                    "expected the program to end with an uncaught exception reporting \
                     `{uncaught}`, as node does, but it ran to completion"
                ),
                started,
            );
            result.stdout = stdout;
            return result;
        }
        (Ok(()), None) => {}
    }
    judge_stdout(stdout, expected.stdout, started)
}

fn judge_stdout(stdout: String, expected: &str, started: Instant) -> CaseResult {
    if stdout == expected {
        return CaseResult::new(Outcome::Pass, Vec::new(), stdout, started);
    }
    let mut result = failed(
        DiagnosticPhase::Expectation,
        mismatch_message(expected, &stdout),
        started,
    );
    result.stdout = stdout;
    result
}

/// The ECMAScript goal a corpus program is parsed under, decided by extension and nothing else.
///
/// `.js` is a Script.  Most of this corpus is, its expected output was observed from a script,
/// and choosing the module goal for all of it would let module-only syntax be silently accepted
/// in programs that were never run that way.
///
/// `.mjs` is a Module, because in Node that extension *is* the declaration and the server probes
/// use it to make one.  Note what this deliberately does not do: Node also takes the goal from
/// the nearest package.json `type` field, and nothing here reads it, so a `.js` file in a
/// `"type": "module"` package is a Script to Kiln and a Module to Node.  That divergence is a
/// work item for the compatibility graft (design.md §14 step 10) rather than something to paper
/// over in the harness, and `conformance/manifest.toml`'s `[server]` table records it as one.
fn source_goal(path: &Path) -> SourceGoal {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("mjs") => SourceGoal::Module,
        _ => SourceGoal::Script,
    }
}

fn source_goal_for_node_format(format: NodeModuleFormat) -> SourceGoal {
    match format {
        NodeModuleFormat::EsModule => SourceGoal::Module,
        NodeModuleFormat::CommonJs => SourceGoal::Script,
    }
}

fn is_javascript_source(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "js" || extension == "mjs" || extension == "cjs")
}

/// The reason a program cannot be run in process, if there is one.
///
/// The Effect half of the corpus is TypeScript that imports a library, and the corpus builds each
/// program into a single self-contained file with `npm ci` and esbuild.  Neither is available
/// here, so these are skipped with the reason attached rather than failed: a failure implies Kiln
/// was asked something and got it wrong, and no part of Kiln was asked anything.
///
/// The test is the file extension, not the manifest's `status` field.  Reading `status` would let
/// the file that records an expectation also decide whether the expectation is checked.
fn out_of_process_build_requirement(path: &Path) -> Option<&'static str> {
    let is_javascript = path
        .extension()
        .is_some_and(|extension| extension == "js" || extension == "mjs" || extension == "cjs");
    (!is_javascript).then_some(
        "TypeScript entry point: this program runs only as a single-file bundle, which \
         conformance/effect/build.sh produces out of process",
    )
}

/// The reason a program's packages are missing, if they are.
///
/// `conformance/server/packages/` holds probes that import a framework by bare specifier on
/// purpose, to exercise ADR 0027's resolution policy against a real dependency tree. Its
/// `node_modules` is produced by `npm ci` and gitignored for the same reason `effect/bundle/` is,
/// so a fresh clone has the probes and not their dependencies. Without this the resolver gives its
/// honest answer -- `could not find package `fastify`` -- and the runner records it as a
/// *failure*, which asserts that Kiln was asked something and got it wrong. Kiln was not asked
/// anything: the input is absent. Two probes then fail on every clean checkout, which is the
/// "green tick on an empty measurement" this corpus refuses, pointing the other way.
///
/// Like the bundle check above, this reads the working tree and not the manifest. Whether
/// `node_modules` is there is a fact; `status = "needs-packages"` is an expectation, and letting
/// the file that records an expectation decide whether that expectation is checked is the one
/// thing `manifest.toml` says it must never do.
///
/// Deliberately narrow, in two ways, because both were wrong on the first attempt:
///
/// It fires only when the whole `node_modules` is missing, so a directory that has one and lacks
/// the package the probe imports still fails, and still says so.
///
/// And it fires only when the manifest *declares* dependencies. A `package.json` is not evidence
/// of any: node-mode tests use `{"type":"module"}` purely to classify a `.js` file's format, and
/// treating that as "packages expected" skipped three of them — turning a passing test green in
/// the one way this corpus most dislikes, by not running it.
fn uninstalled_packages_requirement(path: &Path) -> Option<String> {
    let directory = path.parent()?;
    if directory.join("node_modules").is_dir() {
        return None;
    }
    let manifest = fs::read_to_string(directory.join("package.json")).ok()?;
    let declares_dependencies = manifest
        .split_once("\"dependencies\"")
        .and_then(|(_, rest)| rest.split_once('{'))
        .and_then(|(_, body)| body.split_once('}'))
        .is_some_and(|(entries, _)| entries.contains('"'));
    if !declares_dependencies {
        return None;
    }
    Some(format!(
        "packages are not installed: this program imports by bare specifier and runs only where \
         `npm ci` has been run in {}",
        directory.display()
    ))
}

fn skipped(reason: &str, started: Instant) -> CaseResult {
    CaseResult::new(
        Outcome::Skipped,
        vec![Diagnostic::new(DiagnosticPhase::Harness, reason, None)],
        String::new(),
        started,
    )
}

fn failed(phase: DiagnosticPhase, message: impl Into<String>, started: Instant) -> CaseResult {
    failed_with(vec![Diagnostic::new(phase, message, None)], started)
}

fn failed_with(diagnostics: Vec<Diagnostic>, started: Instant) -> CaseResult {
    CaseResult::new(Outcome::Fail, diagnostics, String::new(), started)
}

/// A one-line diff.  Both sides are escaped and elided so that a report with one line per case
/// stays greppable even when a program prints far more than it should.
fn mismatch_message(expected: &str, actual: &str) -> String {
    format!(
        "stdout mismatch: expected {}, got {}",
        excerpt(expected),
        excerpt(actual)
    )
}

fn excerpt(text: &str) -> String {
    let mut end = DIFF_EXCERPT_BYTES.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end == text.len() {
        format!("{text:?}")
    } else {
        format!("{:?}…", &text[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stack every thread in this process that runs a *program* asks for.
    ///
    /// This mirrors `kiln_daemon::SERVE_STACK_BYTES` rather than importing it: `kiln-daemon`
    /// depends on this crate, so the import would be a cycle. Keep the two in step -- they are
    /// the same number for the same reason.
    const PROGRAM_STACK_BYTES: usize = 16 * 1024 * 1024;

    /// Run a closure on a thread sized for a program, and give back what it returned.
    ///
    /// libtest hands each test Rust's 2 MiB default. That is not enough room for a compiler
    /// recursing over its input or a built-in calling back into JavaScript, which is why the
    /// daemon's connection threads, the one-shot server and the corpus pool all ask for more.
    /// A test that runs a real program through the production graph is doing exactly what those
    /// threads do, and was the last place still taking the default.
    ///
    /// It does not fail as a test failure. The stack overflow aborts the whole binary --
    /// `fatal runtime error: stack overflow`, SIGABRT -- so every other test in the crate goes
    /// down with it and the diagnostic names no test. `fastify_server_case_...` reached it first
    /// after an unrelated refactor grew a frame on the property path by a few hundred bytes; the
    /// rest of these tests run the same graph and were behind it in the same queue.
    ///
    /// Scoped, so a case can keep borrowing the paths and strings it built on the test's stack.
    fn on_a_program_stack<T: Send>(body: impl FnOnce() -> T + Send) -> T {
        std::thread::scope(|scope| {
            std::thread::Builder::new()
                .stack_size(PROGRAM_STACK_BYTES)
                .spawn_scoped(scope, body)
                .expect("a thread for the program under test")
                .join()
                .expect("the program under test did not panic")
        })
    }

    /// The tests' own front door to [`super::run_node_against_expected_stdout`], which shadows it
    /// so that every case in this module gets the stack above without naming it. The production
    /// caller in `discovery.rs` already runs on the corpus pool and needs no wrapper.
    fn run_node_against_expected_stdout(
        path: &Path,
        expected: &str,
        mode: RunMode,
    ) -> VariantResult {
        on_a_program_stack(|| super::run_node_against_expected_stdout(path, expected, mode))
    }

    fn run_node_against_expectation(
        path: &Path,
        expected: Expectation<'_>,
        mode: RunMode,
    ) -> VariantResult {
        on_a_program_stack(|| super::run_node_against_expectation(path, expected, mode))
    }

    #[test]
    fn only_javascript_runs_in_process() {
        assert!(out_of_process_build_requirement(Path::new("guard/g01.js")).is_none());
        assert!(out_of_process_build_requirement(Path::new("bundle/01.mjs")).is_none());
        let reason = out_of_process_build_requirement(Path::new("effect/src/01-core.ts"))
            .expect("TypeScript cannot be run without a bundle step");
        assert!(reason.contains("build.sh"), "{reason}");
        assert_eq!(
            out_of_process_build_requirement(Path::new("effect/src/02-gen.ts")),
            Some(reason),
            "every skipped program shares one reason, so a report groups them into one line"
        );
    }

    /// A probe whose dependencies were never installed was not asked anything, so it skips.
    ///
    /// The corpus's rule is that a failure asserts Kiln got something wrong, and two probes in
    /// `server/packages/` failed on every fresh clone for want of a gitignored `node_modules`.
    /// The check reads the working tree rather than `status = "needs-packages"`, so this exercises
    /// it the same way -- by building the two trees and asking, not by declaring an expectation.
    #[test]
    fn a_probe_without_its_packages_skips_rather_than_failing() {
        let root = std::env::temp_dir().join(format!(
            "kiln-conformance-packages-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let probe = root.join("s04.mjs");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create the probe directory");
        fs::write(&probe, "import x from \"fastify\";\n").expect("write the probe");

        // No package.json at all: an ordinary probe, and nothing to say about it.
        assert!(uninstalled_packages_requirement(&probe).is_none());

        // A manifest that declares no dependencies is not evidence of any. Node-mode tests write
        // exactly this to classify a `.js` file's format, and skipping them would hide three
        // passing tests behind a "skipped" that nobody asked for. This is a regression pin.
        fs::write(root.join("package.json"), r#"{"type":"module"}"#)
            .expect("write a manifest with no dependencies");
        assert!(uninstalled_packages_requirement(&probe).is_none());

        // Declared dependencies, none installed: the input is missing, so this is a skip.
        fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"fastify":"5.12.1"}}"#,
        )
        .expect("write the package manifest");
        let reason =
            uninstalled_packages_requirement(&probe).expect("uninstalled packages are a skip");
        assert!(reason.contains("npm ci"), "{reason}");

        // Installed: the runner must proceed, so that a genuinely missing package still fails.
        fs::create_dir_all(root.join("node_modules")).expect("create node_modules");
        assert!(uninstalled_packages_requirement(&probe).is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_diff_stays_on_one_line_however_much_the_program_printed() {
        let message = mismatch_message("want\n", "got\n");
        assert_eq!(
            message,
            "stdout mismatch: expected \"want\\n\", got \"got\\n\""
        );
        assert!(!message.contains('\n'));

        let flood = "x".repeat(DIFF_EXCERPT_BYTES * 4);
        let elided = mismatch_message("", &flood);
        assert!(!elided.contains('\n'));
        assert!(elided.ends_with('…'), "{elided}");
        assert!(elided.len() < flood.len(), "a flood of output is elided");
    }

    #[test]
    fn eliding_never_splits_a_character() {
        let wide = "é".repeat(DIFF_EXCERPT_BYTES);
        assert!(excerpt(&wide).ends_with('…'), "the excerpt is elided");
    }

    #[test]
    fn node_mode_runs_fs_promises_to_quiescence() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("kiln-node-fs-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&root).expect("fixture directory is created");
        let script = root.join("case.js");
        let data = root.join("data.txt");
        let source = format!(
            r#"const fs = process.getBuiltinModule('node:fs/promises');
fs.writeFile({path:?}, 'hello tokio').then(() => fs.readFile({path:?})).then((bytes) => {{
  console.log(bytes.length);
  console.log(bytes[0]);
  return fs.stat({path:?});
}}).then((stat) => {{
  console.log(stat.size);
  console.log(stat.isFile());
  console.log(stat.isDirectory());
}});"#,
            path = data.to_string_lossy()
        );
        fs::write(&script, source).expect("fixture script is written");

        let result =
            run_node_against_expected_stdout(&script, "11\n104\n11\ntrue\nfalse\n", RunMode::Node);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(result.result.outcome, Outcome::Pass, "{result:?}");
    }

    #[test]
    fn node_mode_drains_http_continuations_before_waiting_for_referenced_work() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kiln-node-http-keep-alive-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("fixture directory is created");
        let entry = root.join("entry.mjs");
        fs::write(
            &entry,
            r#"import http from 'node:http';

const server = http.createServer((request, response) => {
  response.end(request.url);
});

function get(port, path) {
  return new Promise(resolve => {
    http.get({ host: '127.0.0.1', port, path }, response => {
      response.setEncoding('utf8');
      let body = '';
      response.on('data', chunk => { body += chunk; });
      response.on('end', () => resolve(body));
    });
  });
}

server.listen(0, '127.0.0.1', async () => {
  const port = server.address().port;
  console.log(await get(port, '/one'));
  console.log(await get(port, '/two'));
  console.log(await get(port, '/three'));
  server.close();
});
"#,
        )
        .expect("keep-alive fixture is written");

        let result =
            run_node_against_expected_stdout(&entry, "/one\n/two\n/three\n", RunMode::Node);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(result.result.outcome, Outcome::Pass, "{result:?}");
    }

    #[test]
    fn node_mode_keeps_runtime_http_readable_flow_control_in_hyper() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kiln-node-http-readable-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("fixture directory is created");
        let entry = root.join("entry.mjs");
        fs::write(
            &entry,
            r#"import http from 'node:http';

const server = http.createServer((request, response) => {
  request.pause();
  let body = '';
  request.setEncoding('utf8');
  request.on('data', chunk => { body += chunk; });
  request.on('end', () => response.end(body));
  setTimeout(() => request.resume(), 1);
});

server.listen(0, '127.0.0.1', () => {
  const request = http.request({
    host: '127.0.0.1',
    port: server.address().port,
    method: 'POST',
    path: '/',
    headers: { 'content-length': '5' },
  }, response => {
    response.setEncoding('utf8');
    let body = '';
    response.on('data', chunk => { body += chunk; });
    response.on('end', () => {
      console.log(body);
      server.close();
    });
  });
  request.end('hello');
});
"#,
        )
        .expect("readable fixture is written");

        let result = run_node_against_expected_stdout(&entry, "hello\n", RunMode::Node);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(result.result.outcome, Outcome::Pass, "{result:?}");
    }

    #[test]
    fn node_mode_uses_package_type_for_js_modules() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kiln-node-package-type-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("fixture directory is created");
        fs::write(root.join("package.json"), r#"{"type":"module"}"#)
            .expect("package manifest is written");
        let entry = root.join("entry.js");
        fs::write(root.join("dependency.js"), "export const answer = 42;\n")
            .expect("module dependency is written");
        fs::write(
            &entry,
            "import { answer } from './dependency.js'; console.log(answer);\n",
        )
        .expect("module entry is written");

        let parse_result = run_node_against_expected_stdout(&entry, "", RunMode::Parse);
        let result = run_node_against_expected_stdout(&entry, "42\n", RunMode::Node);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            parse_result.result.outcome,
            Outcome::Pass,
            "{parse_result:?}"
        );
        assert_eq!(result.result.outcome, Outcome::Pass, "{result:?}");
    }

    /// An expected uncaught exception is judged both ways: the recorded one passes, with the
    /// stdout before it still compared, and a run that completes instead fails.
    #[test]
    fn an_expected_uncaught_exception_is_part_of_the_answer() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kiln-node-expected-uncaught-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("fixture directory is created");
        let rejects = root.join("rejects.js");
        let completes = root.join("completes.js");
        fs::write(
            &rejects,
            "console.log(1);\nPromise.reject(new Error('boom'));\n",
        )
        .expect("fixture is written");
        fs::write(&completes, "console.log(1);\n").expect("fixture is written");
        let expected = Expectation {
            stdout: "1\n",
            uncaught: Some("Error: boom"),
        };
        let recorded = run_node_against_expectation(&rejects, expected, RunMode::Node);
        let wrong_stdout = run_node_against_expectation(
            &rejects,
            Expectation {
                stdout: "2\n",
                ..expected
            },
            RunMode::Node,
        );
        let completed = run_node_against_expectation(&completes, expected, RunMode::Node);
        let unexpected = run_node_against_expected_stdout(&rejects, "1\n", RunMode::Node);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(recorded.result.outcome, Outcome::Pass, "{recorded:?}");
        assert_eq!(
            wrong_stdout.result.outcome,
            Outcome::Fail,
            "{wrong_stdout:?}"
        );
        assert_eq!(completed.result.outcome, Outcome::Fail, "{completed:?}");
        assert_eq!(unexpected.result.outcome, Outcome::Fail, "{unexpected:?}");
    }

    #[test]
    fn node_mode_uses_module_for_mjs_by_extension() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kiln-node-mjs-module-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("fixture directory is created");
        let entry = root.join("entry.mjs");
        fs::write(&entry, "export const answer = 42; console.log(answer);\n")
            .expect("module entry is written");

        let result = run_node_against_expected_stdout(&entry, "42\n", RunMode::Node);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(result.result.outcome, Outcome::Pass, "{result:?}");
    }

    #[test]
    fn node_mode_uses_commonjs_for_package_type_and_cjs() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kiln-node-commonjs-default-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("fixture directory is created");
        fs::write(root.join("package.json"), r#"{"type":"commonjs"}"#)
            .expect("package manifest is written");
        fs::write(
            root.join("dependency.cjs"),
            "module.exports = { answer: 42 };\n",
        )
        .expect("CommonJS dependency is written");
        let entry = root.join("entry.js");
        fs::write(
            &entry,
            "const dependency = require('./dependency.cjs'); console.log(dependency.answer);\n",
        )
        .expect("CommonJS entry is written");

        let result = run_node_against_expected_stdout(&entry, "42\n", RunMode::Node);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(result.result.outcome, Outcome::Pass, "{result:?}");
    }

    #[test]
    fn node_parse_rejects_top_level_await_in_cjs() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kiln-node-cjs-parse-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("fixture directory is created");
        let entry = root.join("entry.cjs");
        fs::write(&entry, "await Promise.resolve();\n").expect("CommonJS entry is written");

        let result = run_node_against_expected_stdout(&entry, "", RunMode::Parse);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(result.result.outcome, Outcome::Fail, "{result:?}");
        assert_eq!(
            result.result.diagnostics[0].phase,
            DiagnosticPhase::Parse,
            "{result:?}"
        );
    }

    #[test]
    fn node_mode_reclassifies_js_after_package_manifest_deletion() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kiln-node-package-delete-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("fixture directory is created");
        let package = root.join("package.json");
        fs::write(&package, r#"{"type":"module"}"#).expect("package manifest is written");
        fs::write(root.join("dependency.js"), "export const answer = 42;\n")
            .expect("module dependency is written");
        let entry = root.join("entry.js");
        fs::write(
            &entry,
            "import { answer } from './dependency.js'; console.log(answer);\n",
        )
        .expect("module entry is written");

        let module_result = run_node_against_expected_stdout(&entry, "42\n", RunMode::Node);
        fs::remove_file(&package).expect("package manifest is deleted");
        let commonjs_result = run_node_against_expected_stdout(&entry, "", RunMode::Parse);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(
            module_result.result.outcome,
            Outcome::Pass,
            "{module_result:?}"
        );
        assert_eq!(
            commonjs_result.result.outcome,
            Outcome::Fail,
            "{commonjs_result:?}"
        );
        assert_eq!(
            commonjs_result.result.diagnostics[0].phase,
            DiagnosticPhase::Parse,
            "{commonjs_result:?}"
        );
    }

    #[test]
    fn pinned_node_http_server_client_case_runs_through_the_production_graph() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../conformance/node");
        let test = crate::discover(&root)
            .expect("discover pinned Node corpus")
            .into_iter()
            .find(|test| test.id() == "node-v24.7.0/http/tokio-owned-server-client")
            .expect("pinned HTTP server/client case is present");

        let results = test.run(RunMode::Node);
        assert_eq!(results.len(), 1, "manifest cases have one execution");
        assert_eq!(results[0].result.outcome, Outcome::Pass, "{results:#?}");
        assert_eq!(results[0].result.stdout, "pass\n");
    }

    #[test]
    fn pinned_node_net_close_projects_closed_state_before_the_event() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../conformance/node");
        let test = crate::discover(&root)
            .expect("discover pinned Node corpus")
            .into_iter()
            .find(|test| test.id() == "node-v24.7.0/net/socket-set-encoding")
            .expect("pinned node:net socket case is present");

        let results = test.run(RunMode::Node);
        assert_eq!(results.len(), 1, "manifest cases have one execution");
        assert_eq!(results[0].result.outcome, Outcome::Pass, "{results:#?}");
        assert_eq!(results[0].result.stdout, "pass\n");
    }

    #[test]
    fn fastify_server_case_runs_through_the_production_graph_when_installed() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../conformance/server/packages");
        if !root.join("node_modules/fastify/fastify.js").is_file() {
            return;
        }

        let entry = root.join("s04-fastify.mjs");
        let result = run_node_against_expected_stdout(
            &entry,
            "200 {\"ok\":true}\n200 {\"id\":\"7\"}\n200 {\"got\":{\"hello\":\"kiln\"}}\n404 {\"error\":\"not found\"}\n",
            RunMode::Node,
        );
        assert_eq!(result.result.outcome, Outcome::Pass, "{result:#?}");
    }
}
