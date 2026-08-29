//! The Test262 metadata source: tests that carry their own `/*--- … ---*/` frontmatter.
//!
//! This is Kiln's default convention for tests it writes itself, and the only source that can
//! express Test262's negative-test and strictness contracts.  It has no process-global VM or
//! host: every invocation creates a new host and asks the interpreter for a new VM, which keeps
//! globals and captured output from leaking between cases.  That is the same execution path as
//! the CLI's development mode rather than a second conformance-only implementation.

use std::time::Instant;

use frontend_oxc::parse_with_goal;
use kiln_hir::{SourceGoal, Span};
use kiln_interpreter::{ExecutionError, ExecutionLimits, execute};
use kiln_runtime::CaptureHost;
use kiln_test262::{Phase, TestCase};

use crate::case::{
    CaseResult, Diagnostic, DiagnosticPhase, Outcome, RunMode, Variant, VariantResult,
};

/// Execute one Test262 case in the requested mode.
pub fn run_case(case: &TestCase, mode: RunMode) -> CaseResult {
    run_variant(case, mode, default_variant(case))
}

/// Run all Test262 strictness variants applicable to a case.
///
/// The ordinary Test262 contract runs a test once as sloppy and once with a strict directive.
/// Callers that aggregate conformance results should use this entry point; [`run_case`] remains a
/// convenient single-variant API for targeted tests and diagnostics.
pub fn run_case_variants(case: &TestCase, mode: RunMode) -> Vec<VariantResult> {
    variants(case)
        .into_iter()
        .map(|variant| VariantResult {
            variant,
            result: run_variant(case, mode, variant),
        })
        .collect()
}

/// Run one explicit Test262 realm variant.
pub fn run_variant(case: &TestCase, mode: RunMode, variant: Variant) -> CaseResult {
    let started = Instant::now();
    let mut diagnostics = Vec::new();

    // Async flags alter the Test262 realm contract.  Treating their source as an ordinary script
    // produces deceptively green tests, so make the decision explicit.  Raw tests are deliberately
    // allowed through unchanged; module tests are parsed with their explicit goal and rejected
    // below only when execution would require linking.  `includes` is handled as unsupported too:
    // this runner does not have the Test262 assertion harness loaded yet.
    if matches!(mode, RunMode::Interpreter)
        && let Some(reason) = unsupported_case_requirement(case)
    {
        diagnostics.push(Diagnostic::new(DiagnosticPhase::Harness, reason, None));
        return CaseResult::new(Outcome::Skipped, diagnostics, String::new(), started);
    }

    let path = case.path.to_string_lossy();
    // Test262 uses `.js` for both scripts and modules.  Always choosing the script goal for the
    // ordinary case avoids accidentally accepting module-only syntax; the module flag is the one
    // explicit exception and is parsed with the module goal before the linker limitation is
    // reported below.
    let module_flag = has_flag(case, "module") || matches!(variant, Variant::Module);
    let goal = if module_flag {
        SourceGoal::Module
    } else {
        SourceGoal::Script
    };
    let (source, prefix_len) = match variant {
        Variant::Strict => (
            format!("\"use strict\";\n{}", case.source),
            "\"use strict\";\n".len(),
        ),
        Variant::Sloppy | Variant::Module => (case.source.clone(), 0),
    };
    let module = parse_with_goal(path.as_ref(), &source, goal);
    diagnostics.extend(module.diagnostics.iter().map(|diagnostic| {
        Diagnostic::new(
            DiagnosticPhase::Parse,
            diagnostic.message.clone(),
            diagnostic.span.map(|span| adjust_span(span, prefix_len)),
        )
    }));

    let negative = case.metadata.negative.as_ref();
    let is_negative_parse =
        negative.is_some_and(|negative| matches!(&negative.phase, Phase::Parse));

    // A parse-negative case passes only when Oxc actually emitted parser diagnostics.  A
    // recoverable HIR or a frontend panic is not a substitute for a parser diagnostic here.
    if matches!(mode, RunMode::Parse) {
        let outcome = if is_negative_parse {
            if module.diagnostics.is_empty() {
                diagnostics.push(Diagnostic::new(
                    DiagnosticPhase::Parse,
                    "expected a parser diagnostic for a parse-negative case",
                    None,
                ));
                Outcome::Fail
            } else if parse_error_type_matches(
                negative.expect("negative parse checked above"),
                &module.diagnostics,
            ) {
                Outcome::Pass
            } else {
                diagnostics.push(Diagnostic::new(
                    DiagnosticPhase::Parse,
                    format!(
                        "parser diagnostics could not verify expected error type `{}`",
                        negative.expect("negative parse checked above").type_name
                    ),
                    None,
                ));
                Outcome::Unsupported
            }
        } else if negative.is_some() {
            diagnostics.push(Diagnostic::new(
                DiagnosticPhase::Harness,
                "negative phase is not observable in parse mode",
                None,
            ));
            Outcome::Unsupported
        } else if module.diagnostics.is_empty() && !module.panicked {
            Outcome::Pass
        } else {
            if module.panicked {
                diagnostics.push(Diagnostic::new(
                    DiagnosticPhase::Parse,
                    "frontend parser could not recover a program",
                    None,
                ));
            }
            Outcome::Fail
        };
        return CaseResult::new(outcome, diagnostics, String::new(), started);
    }

    // Parse diagnostics always win in interpreter mode.  In particular, a runtime-negative case
    // must not be marked as passing merely because malformed source later fails to compile.
    if !module.diagnostics.is_empty() || module.panicked {
        if module.panicked && module.diagnostics.is_empty() {
            diagnostics.push(Diagnostic::new(
                DiagnosticPhase::Parse,
                "frontend parser could not recover a program",
                None,
            ));
        }
        let outcome = if is_negative_parse {
            if module.diagnostics.is_empty() {
                Outcome::Fail
            } else if parse_error_type_matches(
                negative.expect("negative parse checked above"),
                &module.diagnostics,
            ) {
                Outcome::Pass
            } else {
                diagnostics.push(Diagnostic::new(
                    DiagnosticPhase::Parse,
                    format!(
                        "parser diagnostics could not verify expected error type `{}`",
                        negative.expect("negative parse checked above").type_name
                    ),
                    None,
                ));
                Outcome::Unsupported
            }
        } else {
            Outcome::Fail
        };
        return CaseResult::new(outcome, diagnostics, String::new(), started);
    }

    if module_flag {
        diagnostics.push(Diagnostic::new(
            DiagnosticPhase::Harness,
            "Test262 `module` flag requires module linking, which is not implemented",
            None,
        ));
        return CaseResult::new(Outcome::Unsupported, diagnostics, String::new(), started);
    }

    let Some(negative) = negative else {
        return execute_positive(&module, diagnostics, prefix_len, started);
    };

    // Resolution is a separate Test262 phase.  This vertical slice has no module linker, and a
    // compile diagnostic is not a runtime exception, so claiming a resolution-negative pass
    // would be incorrect.
    if !matches!(&negative.phase, Phase::Runtime) {
        diagnostics.push(Diagnostic::new(
            DiagnosticPhase::Harness,
            format!(
                "negative phase `{:?}` is not supported by the interpreter runner",
                negative.phase
            ),
            None,
        ));
        return CaseResult::new(Outcome::Unsupported, diagnostics, String::new(), started);
    }

    let program = match kiln_ir::compile(&module) {
        Ok(program) => program,
        Err(compile_diagnostics) => {
            let unsupported = compile_diagnostics
                .iter()
                .all(|diagnostic| is_intentionally_unsupported(&diagnostic.message));
            diagnostics.extend(compile_diagnostics.into_iter().map(|diagnostic| {
                Diagnostic::new(
                    DiagnosticPhase::Compile,
                    diagnostic.message,
                    diagnostic.span.map(|span| adjust_span(span, prefix_len)),
                )
            }));
            // Unsupported HIR is never the runtime error promised by a negative test.  Keep this
            // distinct from Pass so capability gaps cannot hide as conformance successes.
            return CaseResult::new(
                if unsupported {
                    Outcome::Unsupported
                } else {
                    Outcome::Fail
                },
                diagnostics,
                String::new(),
                started,
            );
        }
    };

    let mut host = CaptureHost::new();
    match execute(&program, &mut host, ExecutionLimits::default()) {
        Ok(_) => {
            diagnostics.push(Diagnostic::new(
                DiagnosticPhase::Runtime,
                "expected a runtime exception for a runtime-negative case",
                None,
            ));
            CaseResult::new(Outcome::Fail, diagnostics, host.into_stdout(), started)
        }
        Err(error) => {
            diagnostics.push(runtime_diagnostic(&error));
            // This scalar interpreter does not yet expose JavaScript Error constructors at the
            // host boundary.  A runtime failure therefore cannot verify a declared Test262 error
            // type; classifying it Unsupported is safer than claiming a conformance pass.
            if runtime_error_type_matches(negative.type_name.as_str(), &error) {
                CaseResult::new(Outcome::Pass, diagnostics, host.into_stdout(), started)
            } else {
                diagnostics.push(Diagnostic::new(
                    DiagnosticPhase::Runtime,
                    format!(
                        "runtime error could not verify expected error type `{}`",
                        negative.type_name
                    ),
                    None,
                ));
                CaseResult::new(
                    Outcome::Unsupported,
                    diagnostics,
                    host.into_stdout(),
                    started,
                )
            }
        }
    }
}

fn execute_positive(
    module: &kiln_hir::Module,
    mut diagnostics: Vec<Diagnostic>,
    prefix_len: usize,
    started: Instant,
) -> CaseResult {
    let program = match kiln_ir::compile(module) {
        Ok(program) => program,
        Err(compile_diagnostics) => {
            let unsupported = compile_diagnostics
                .iter()
                .all(|diagnostic| is_intentionally_unsupported(&diagnostic.message));
            diagnostics.extend(compile_diagnostics.into_iter().map(|diagnostic| {
                Diagnostic::new(
                    DiagnosticPhase::Compile,
                    diagnostic.message,
                    diagnostic.span.map(|span| adjust_span(span, prefix_len)),
                )
            }));
            return CaseResult::new(
                if unsupported {
                    Outcome::Unsupported
                } else {
                    Outcome::Fail
                },
                diagnostics,
                String::new(),
                started,
            );
        }
    };

    let mut host = CaptureHost::new();
    match execute(&program, &mut host, ExecutionLimits::default()) {
        Ok(_) => CaseResult::new(Outcome::Pass, diagnostics, host.into_stdout(), started),
        Err(error) => {
            diagnostics.push(runtime_diagnostic(&error));
            CaseResult::new(Outcome::Fail, diagnostics, host.into_stdout(), started)
        }
    }
}

fn runtime_diagnostic(error: &ExecutionError) -> Diagnostic {
    Diagnostic::new(DiagnosticPhase::Runtime, error.to_string(), None)
}

fn default_variant(case: &TestCase) -> Variant {
    if has_flag(case, "module") {
        Variant::Module
    } else if has_flag(case, "onlyStrict") {
        Variant::Strict
    } else {
        Variant::Sloppy
    }
}

fn variants(case: &TestCase) -> Vec<Variant> {
    if has_flag(case, "module") {
        vec![Variant::Module]
    } else if has_flag(case, "raw") {
        // Raw tests opt out of Test262's strict/sloppy wrapper expansion and execute exactly as
        // supplied by the fixture.
        vec![Variant::Sloppy]
    } else if has_flag(case, "onlyStrict") {
        vec![Variant::Strict]
    } else if has_flag(case, "noStrict") {
        vec![Variant::Sloppy]
    } else {
        vec![Variant::Sloppy, Variant::Strict]
    }
}

fn adjust_span(span: Span, prefix_len: usize) -> Span {
    let prefix_len = prefix_len as u32;
    Span::new(
        span.start.saturating_sub(prefix_len),
        span.end.saturating_sub(prefix_len),
    )
}

/// Oxc's parser diagnostics are syntax errors.  It does not currently expose a constructor name,
/// so `SyntaxError` is the only declared Test262 type this runner can verify for this phase.
fn parse_error_type_matches(
    negative: &kiln_test262::NegativeExpectation,
    _diagnostics: &[kiln_hir::Diagnostic],
) -> bool {
    negative.type_name == "SyntaxError"
}

/// Runtime errors currently have no JavaScript constructor identity.  Keep this helper separate
/// so a future interpreter Error value can add a real matcher without changing classification.
fn runtime_error_type_matches(_expected: &str, _error: &ExecutionError) -> bool {
    false
}

fn is_intentionally_unsupported(message: &str) -> bool {
    message.starts_with("unsupported statement:")
        || message.starts_with("unsupported expression:")
        || message.starts_with("unsupported member access")
        || message.starts_with("only console.log(")
        || message.starts_with("module dependency `")
        || message.starts_with("re-export from `")
}

fn unsupported_case_requirement(case: &TestCase) -> Option<String> {
    for flag in &case.metadata.flags {
        let flag = flag.as_str();
        let reason = match flag {
            "async" => "Test262 `async` flag requires an async event-loop driver",
            _ => continue,
        };
        return Some(reason.to_owned());
    }
    if !case.metadata.includes.is_empty() {
        return Some("Test262 harness includes are not loaded by this runner".to_owned());
    }
    if !has_flag(case, "raw") {
        return Some(
            "Test262 default assert.js/sta.js harness is not loaded by this runner".to_owned(),
        );
    }
    None
}

fn has_flag(case: &TestCase, wanted: &str) -> bool {
    case.metadata
        .flags
        .iter()
        .any(|flag| flag.as_str() == wanted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_test262::{Metadata, NegativeExpectation};
    use std::path::PathBuf;

    fn case(source: &str) -> TestCase {
        TestCase {
            path: PathBuf::from("fixture.js"),
            source: source.to_owned(),
            metadata: Metadata::default(),
        }
    }

    #[test]
    fn positive_parse_passes_and_parse_negative_requires_a_diagnostic() {
        assert_eq!(
            run_case(&case("const answer = 42;"), RunMode::Parse).outcome,
            Outcome::Pass
        );

        let mut negative = case("const = ;");
        negative.metadata.negative = Some(NegativeExpectation {
            phase: Phase::Parse,
            type_name: "SyntaxError".to_owned(),
        });
        assert_eq!(run_case(&negative, RunMode::Parse).outcome, Outcome::Pass);

        let mut missing = case("const answer = 42;");
        missing.metadata.negative = Some(NegativeExpectation {
            phase: Phase::Parse,
            type_name: "SyntaxError".to_owned(),
        });
        assert_eq!(run_case(&missing, RunMode::Parse).outcome, Outcome::Fail);

        let mut wrong_type = case("const = ;");
        wrong_type.metadata.negative = Some(NegativeExpectation {
            phase: Phase::Parse,
            type_name: "TypeError".to_owned(),
        });
        assert_eq!(
            run_case(&wrong_type, RunMode::Parse).outcome,
            Outcome::Unsupported
        );
    }

    #[test]
    fn unsupported_hir_is_not_a_negative_runtime_pass() {
        let mut negative = case("function f() {}\n");
        negative.metadata.flags.push("raw".to_owned());
        negative.metadata.negative = Some(NegativeExpectation {
            phase: Phase::Runtime,
            type_name: "TypeError".to_owned(),
        });
        let result = run_case(&negative, RunMode::Interpreter);
        assert_eq!(result.outcome, Outcome::Unsupported);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("unsupported statement"))
        );
    }

    #[test]
    fn each_interpreter_case_gets_fresh_host_state() {
        let mut first_case = case("console.log('one');");
        first_case.metadata.flags.push("raw".to_owned());
        let mut second_case = case("console.log('two');");
        second_case.metadata.flags.push("raw".to_owned());
        let first = run_case(&first_case, RunMode::Interpreter);
        let second = run_case(&second_case, RunMode::Interpreter);
        assert_eq!(first.outcome, Outcome::Pass);
        assert_eq!(second.outcome, Outcome::Pass);
        assert_eq!(first.stdout, "one\n");
        assert_eq!(second.stdout, "two\n");
    }

    #[test]
    fn special_flags_are_explicitly_skipped() {
        let mut flagged = case("console.log('must not run');");
        flagged.metadata.flags.push("async".to_owned());
        let result = run_case(&flagged, RunMode::Interpreter);
        assert_eq!(result.outcome, Outcome::Skipped);
        assert!(result.stdout.is_empty());
        assert!(result.diagnostics[0].message.contains("async"));
    }

    #[test]
    fn raw_cases_run_without_a_harness_wrapper() {
        let mut raw = case("console.log('raw');");
        raw.metadata.flags.push("raw".to_owned());
        let result = run_case(&raw, RunMode::Interpreter);
        assert_eq!(result.outcome, Outcome::Pass);
        assert_eq!(result.stdout, "raw\n");
        let results = run_case_variants(&raw, RunMode::Interpreter);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].variant, Variant::Sloppy);
    }

    #[test]
    fn strictness_and_module_variants_are_explicit() {
        let ordinary = run_case_variants(&case("const value = 1;"), RunMode::Parse);
        assert_eq!(
            ordinary.iter().map(|item| item.variant).collect::<Vec<_>>(),
            [Variant::Sloppy, Variant::Strict]
        );
        assert!(
            ordinary
                .iter()
                .all(|item| item.result.outcome == Outcome::Pass),
            "parse-only variants do not require the runtime harness"
        );
        assert!(
            run_case_variants(&case("const value = 1;"), RunMode::Interpreter)
                .iter()
                .all(|item| item.result.outcome == Outcome::Skipped),
            "ordinary execution must not omit the default Test262 harness"
        );

        let mut only_strict = case("const value = 1;");
        only_strict.metadata.flags.push("onlyStrict".to_owned());
        assert_eq!(run_case_variants(&only_strict, RunMode::Parse).len(), 1);
        assert_eq!(
            run_case_variants(&only_strict, RunMode::Parse)[0].variant,
            Variant::Strict
        );

        let mut module = case("export const value = 1;");
        module.metadata.flags.push("module".to_owned());
        let module_results = run_case_variants(&module, RunMode::Parse);
        assert_eq!(module_results.len(), 1);
        assert_eq!(module_results[0].variant, Variant::Module);
        assert_eq!(module_results[0].result.outcome, Outcome::Pass);
    }
}
