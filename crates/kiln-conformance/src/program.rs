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
use kiln_interpreter::{ExecutionLimits, execute};
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
    let started = Instant::now();

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

    // These programs are ordinary scripts.  Choosing the script goal keeps module-only syntax
    // from being silently accepted in a corpus whose expected output was observed from a script.
    let display_path = path.to_string_lossy();
    let module = parse_with_goal(display_path.as_ref(), &source, SourceGoal::Script);
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
    if let Err(error) = execute(&program, &mut host, ExecutionLimits::default()) {
        let mut result = failed(DiagnosticPhase::Runtime, error.to_string(), started);
        result.stdout = host.into_stdout();
        return result;
    }

    let stdout = host.into_stdout();
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
        .is_some_and(|extension| extension == "js" || extension == "mjs");
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
}
