//! Running a program file against the stdout a corpus recorded for it.
//!
//! The contract is deliberately narrow, because it is the one the manifest can state: the program
//! runs to completion and writes exactly the recorded bytes, or it does not.  Everything else is a
//! failure that keeps its reason — a lowering refusal, a runtime error, or a diff — because those
//! reasons are the interpreter backlog and collapsing them to a boolean throws the backlog away.

use std::fs;
use std::path::Path;
use std::time::Instant;

use frontend_oxc::parse_with_goal;
use kiln_hir::SourceGoal;
use kiln_interpreter::{ExecutionLimits, execute, execute_node};
use kiln_module_graph::ModuleGraph;
use kiln_module_resolver::NodeRuntimeResolver;
use kiln_runtime::CaptureHost;

use crate::case::{
    CaseResult, Diagnostic, DiagnosticPhase, Outcome, RunMode, Variant, VariantResult,
};

/// How much of either side of a stdout diff a one-line report shows before it elides.
const DIFF_EXCERPT_BYTES: usize = 160;

/// Run `path` and judge its output against `expected`.
///
/// The single reported variant is [`Variant::Sloppy`]: a recorded stdout is an observation of one
/// ordinary script run, and Test262's strict-directive expansion would be judging a program the
/// corpus never observed.
pub(crate) fn run_against_expected_stdout(
    path: &Path,
    expected: &str,
    mode: RunMode,
) -> VariantResult {
    VariantResult {
        variant: Variant::Sloppy,
        result: run(path, expected, mode),
    }
}

fn run(path: &Path, expected: &str, mode: RunMode) -> CaseResult {
    run_with_executor(path, expected, mode, false)
}

/// Run a source file through the same compiler but with the Tokio-owned Node Adapter installed.
pub fn run_node_against_expected_stdout(
    path: &Path,
    expected: &str,
    mode: RunMode,
) -> VariantResult {
    VariantResult {
        variant: Variant::Sloppy,
        result: if source_goal(path) == SourceGoal::Module
            || path.extension().is_some_and(|extension| extension == "cjs")
        {
            run_node_module(path, expected, mode)
        } else {
            run_with_executor(path, expected, mode, true)
        },
    }
}

/// Run a Node module through the production resolver, graph, linker, shared realm session, and
/// Tokio-owned Adapter. Keeping this path in the corpus prevents a single-file test executor from
/// becoming a second, easier-to-pass Node runtime that real programs never use.
fn run_node_module(path: &Path, expected: &str, mode: RunMode) -> CaseResult {
    let started = Instant::now();
    crate::source_compiler::install();

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
        let module = parse_with_goal(&path.to_string_lossy(), &source, SourceGoal::Module);
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

    let project_root = path.parent().unwrap_or_else(|| Path::new("."));
    let resolver = match NodeRuntimeResolver::new(project_root) {
        Ok(resolver) => resolver,
        Err(error) => return failed(DiagnosticPhase::Harness, error.to_string(), started),
    };
    struct NodeFrontend;
    impl kiln_module_graph::Frontend for NodeFrontend {
        fn parse(&mut self, path: &str, source: &str) -> kiln_hir::Module {
            parse_with_goal(path, source, SourceGoal::Module)
        }

        fn parse_commonjs(&mut self, path: &str, source: &str) -> kiln_hir::Module {
            parse_with_goal(path, source, SourceGoal::Script)
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

    let mut host = CaptureHost::new();
    if let Err(error) = graph.execute_node(&mut host, ExecutionLimits::default()) {
        let mut result = failed(DiagnosticPhase::Runtime, error.to_string(), started);
        result.stdout = host.into_stdout();
        return result;
    }
    judge_stdout(host.into_stdout(), expected, started)
}

fn run_with_executor(path: &Path, expected: &str, mode: RunMode, node: bool) -> CaseResult {
    let started = Instant::now();
    crate::source_compiler::install();

    if let Some(reason) = out_of_process_build_requirement(path) {
        return skipped(reason, started);
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

    let program = match kiln_ir::compile(&module) {
        Ok(program) => program,
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

    let mut host = CaptureHost::new();
    let result = if node {
        execute_node(
            std::rc::Rc::new(program),
            &mut host,
            ExecutionLimits::default(),
        )
    } else {
        execute(
            std::rc::Rc::new(program),
            &mut host,
            ExecutionLimits::default(),
        )
    };
    if let Err(error) = result {
        let mut result = failed(DiagnosticPhase::Runtime, error.to_string(), started);
        result.stdout = host.into_stdout();
        return result;
    }

    judge_stdout(host.into_stdout(), expected, started)
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
}
