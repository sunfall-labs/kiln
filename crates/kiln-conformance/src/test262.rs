//! The Test262 metadata source: tests that carry their own `/*--- … ---*/` frontmatter.
//!
//! This is Kiln's default convention for tests it writes itself, and the only source that can
//! express Test262's negative-test and strictness contracts.  It has no process-global VM or
//! host: every invocation creates a new host and asks the interpreter for a new VM, which keeps
//! globals and captured output from leaking between cases.  That is the same execution path as
//! the CLI's development mode rather than a second conformance-only implementation.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use frontend_oxc::parse_with_goal;
use kiln_hir::{SourceGoal, Span};
use kiln_interpreter::{ExecutionError, ExecutionLimits, execute_module, execute_reclaiming};
use kiln_ir::ModuleProgram;
use kiln_module_graph::{ModuleGraph, ModuleGraphError, ResolveError};
use kiln_runtime::CaptureHost;
use kiln_test262::{NegativeExpectation, Phase, TestCase};

use crate::case::{
    CaseResult, Diagnostic, DiagnosticPhase, Outcome, RunMode, Variant, VariantResult,
};
use crate::harness::{
    ASYNC_COMPLETE, ASYNC_FAILURE, ASYNC_INCLUDE, HOST_CAPABILITY_MARKER, HOST_OBJECT_PRELUDE,
    Harness,
};

/// Execute one Test262 case in the requested mode, without an assertion harness.
pub fn run_case(case: &TestCase, mode: RunMode) -> CaseResult {
    run_variant(case, mode, default_variant(case))
}

/// Execute one Test262 case with an assertion harness available.
pub fn run_case_with_harness(
    case: &TestCase,
    mode: RunMode,
    harness: Option<&Harness>,
) -> CaseResult {
    run_variant_with_harness(case, mode, default_variant(case), harness)
}

/// Run all Test262 strictness variants applicable to a case.
///
/// The ordinary Test262 contract runs a test once as sloppy and once with a strict directive.
/// Callers that aggregate conformance results should use this entry point; [`run_case`] remains a
/// convenient single-variant API for targeted tests and diagnostics.
pub fn run_case_variants(case: &TestCase, mode: RunMode) -> Vec<VariantResult> {
    run_case_variants_with_harness(case, mode, None)
}

/// Run all applicable strictness variants with an assertion harness available.
pub fn run_case_variants_with_harness(
    case: &TestCase,
    mode: RunMode,
    harness: Option<&Harness>,
) -> Vec<VariantResult> {
    variants(case)
        .into_iter()
        .map(|variant| VariantResult {
            variant,
            result: run_variant_with_harness(case, mode, variant, harness),
        })
        .collect()
}

/// Run one explicit Test262 realm variant without an assertion harness.
pub fn run_variant(case: &TestCase, mode: RunMode, variant: Variant) -> CaseResult {
    run_variant_with_harness(case, mode, variant, None)
}

/// Run one explicit Test262 realm variant, evaluating the assertion harness first when one is
/// available.
pub fn run_variant_with_harness(
    case: &TestCase,
    mode: RunMode,
    variant: Variant,
    harness: Option<&Harness>,
) -> CaseResult {
    let started = Instant::now();
    // `eval` and `Function` compile source while a program is running, and the interpreter has no
    // frontend of its own to do it with. Installing one here rather than at startup is what keeps
    // this crate's execution path the same one the CLI takes -- a VM per case, nothing global.
    crate::source_compiler::install();
    // A compiler or interpreter bug reached by one test must not take the run with it. Before
    // this, a single `unreachable!` in lowering aborted the process, and a sweep of
    // `language/expressions` reported nothing at all for the other eleven thousand tests in the
    // corpus -- the one result a conformance run must never produce, because it looks like the
    // area was measured. A panic is a failure of that test, recorded like any other.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_variant_uncaught(case, mode, variant, harness, started)
    }));
    match outcome {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panicked".to_owned());
            CaseResult::new(
                Outcome::Fail,
                vec![Diagnostic::new(
                    DiagnosticPhase::Runtime,
                    format!("internal error: {message}"),
                    None,
                )],
                String::new(),
                started,
            )
        }
    }
}

fn run_variant_uncaught(
    case: &TestCase,
    mode: RunMode,
    variant: Variant,
    harness: Option<&Harness>,
    started: Instant,
) -> CaseResult {
    let mut diagnostics = Vec::new();

    let negative = case.metadata.negative.as_ref();
    // A `phase: parse` case asserts that its source is not a program at all.  That makes the
    // parser the whole test: the assertion harness is never reached, the program is never run,
    // and every gate below this point is asking about machinery the case does not use.  Carrying
    // the expectation in its own binding lets those gates step aside for it.
    let parse_negative = negative.filter(|negative| matches!(negative.phase, Phase::Parse));

    // Async flags alter the Test262 realm contract.  Treating their source as an ordinary script
    // produces deceptively green tests, so make the decision explicit.  Raw tests are deliberately
    // allowed through unchanged; module tests are parsed with their explicit goal and rejected
    // below only when execution would require linking.  `includes` is handled as unsupported too:
    // this runner does not have the Test262 assertion harness loaded yet.
    if matches!(mode, RunMode::Interpreter)
        && parse_negative.is_none()
        && let Some(reason) = unsupported_case_requirement(case, harness)
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
    // The assembled realm is: the strict directive (which must lead for it to be a directive
    // prologue), then the harness in Test262's evaluation order, then the test itself.  Spans are
    // reported against the test, so `prefix_len` counts every byte that precedes it.
    let mut prefix = String::new();
    if matches!(variant, Variant::Strict) {
        prefix.push_str("\"use strict\";\n");
    }
    // A parse-negative case is deliberately left out of this: its verdict is the parser's, and
    // prepending thousands of lines of harness only gives the parser more source in which to find
    // an error that is not the one the test is about.
    if matches!(mode, RunMode::Interpreter)
        && parse_negative.is_none()
        && !has_flag(case, "raw")
        && let Some(harness) = harness
        && let Ok(sources) = harness.prelude_for(&case_includes(case))
    {
        for harness_source in sources {
            prefix.push_str(harness_source);
            prefix.push('\n');
        }
        // After the harness, so that a test overriding `$262` still wins, and so the stubs can
        // rely on nothing but the language.
        prefix.push_str(HOST_OBJECT_PRELUDE);
        // `doneprintHandle.js` declares `function $DONE`, and in a Script a top-level function
        // declaration *is* a property of the global object. Kiln keeps top-level bindings in
        // module slots, so `asyncHelpers.js`'s own guard -- `hasOwnProperty(globalThis, "$DONE")`
        // -- was false and every `asyncTest` threw "asyncTest called without async flag" before
        // reaching the behaviour it was written to check. Publishing the binding is the host
        // supplying `$DONE` as INTERPRETING.md requires of it, and it is deliberately the only
        // thing published: the underlying gap, that a script's global bindings are invisible on
        // `globalThis`, is the language's and is left to fail wherever a test asks about it.
        if has_flag(case, "async") {
            prefix.push_str("globalThis.$DONE = $DONE;\n");
        }
    }
    let prefix_len = prefix.len();
    let source = if prefix.is_empty() {
        case.source.clone()
    } else {
        format!("{prefix}{}", case.source)
    };
    let module = parse_with_goal(path.as_ref(), &source, goal);
    diagnostics.extend(module.diagnostics.iter().map(|diagnostic| {
        Diagnostic::new(
            DiagnosticPhase::Parse,
            diagnostic.message.clone(),
            diagnostic.span.map(|span| adjust_span(span, prefix_len)),
        )
    }));

    // A parse-negative case is settled here, in both modes and before every gate below: the
    // parser either rejected the source or it did not, and neither compilation, module linking
    // nor execution can add to that verdict or overturn it.  Deciding it this early is what stops
    // a case the runner cannot *run* -- a module, an unimplemented construct -- from being
    // reported as unsupported when the parser has already answered the only question it asked.
    if let Some(negative) = parse_negative {
        let outcome = parse_negative_outcome(negative, &module, &mut diagnostics);
        return CaseResult::new(outcome, diagnostics, String::new(), started);
    }

    if matches!(mode, RunMode::Parse) {
        let outcome = if negative.is_some() {
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
        return CaseResult::new(Outcome::Fail, diagnostics, String::new(), started);
    }

    if module_flag {
        return execute_module_case(case, negative, &module, diagnostics, prefix_len, started);
    }

    let Some(negative) = negative else {
        return execute_positive(
            has_flag(case, "async"),
            &module,
            conformance_limits(case),
            diagnostics,
            prefix_len,
            started,
        );
    };

    // Resolution is a separate Test262 phase, and the only one still out of reach: it asserts that
    // linking a module graph fails, which needs a linker this vertical slice does not have.  A
    // compile diagnostic is not a link error, so claiming a resolution-negative pass would be
    // asserting something the runner never checked.
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
    match execute_reclaiming(
        std::rc::Rc::new(program),
        &mut host,
        conformance_limits(case),
    ) {
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
    is_async: bool,
    module: &kiln_hir::Module,
    limits: ExecutionLimits,
    diagnostics: Vec<Diagnostic>,
    prefix_len: usize,
    started: Instant,
) -> CaseResult {
    let program = match kiln_ir::compile(module) {
        Ok(program) => program,
        Err(compile_diagnostics) => {
            return compile_failure(compile_diagnostics, diagnostics, prefix_len, started);
        }
    };

    let mut host = CaptureHost::new();
    let outcome = execute_reclaiming(std::rc::Rc::new(program), &mut host, limits).map(|_| ());
    finish_run(outcome, host, is_async, diagnostics, started)
}

/// Run a case whose `module` flag asks for the module goal.
///
/// A module with nothing to import needs the *goal*, not a linker: strict by default, `this`
/// undefined at the top level, and `export` permitted. A module that does import needs both, and
/// its dependencies are ordinary sibling files in the corpus, so the entry program is handed to
/// [`ModuleGraph`] along with a provider that compiles the siblings the graph asks for.
fn execute_module_case(
    case: &TestCase,
    negative: Option<&NegativeExpectation>,
    module: &kiln_hir::Module,
    mut diagnostics: Vec<Diagnostic>,
    prefix_len: usize,
    started: Instant,
) -> CaseResult {
    let program = match kiln_ir::compile_module(module) {
        Ok(program) => program,
        Err(compile_diagnostics) => {
            return compile_failure(compile_diagnostics, diagnostics, prefix_len, started);
        }
    };
    let is_async = has_flag(case, "async");
    let linked = if program.dependencies.is_empty() && program.imports.is_empty() {
        let mut host = CaptureHost::new();
        let outcome =
            execute_module(&program, &[], &mut host, conformance_limits(case)).map(|_| ());
        Ok((outcome, host))
    } else {
        link_and_execute(case, program)
    };

    let (outcome, host) = match linked {
        Ok(ran) => ran,
        Err(failure) => {
            diagnostics.push(Diagnostic::new(
                DiagnosticPhase::Harness,
                failure.message().to_owned(),
                None,
            ));
            let resolution_negative = negative
                .filter(|negative| matches!(negative.phase, Phase::Resolution))
                .filter(|_| matches!(failure, LinkFailure::Syntax(_) | LinkFailure::Link(_)));
            let outcome = match resolution_negative {
                // Resolution is the phase that asserts linking itself fails, and the linker now
                // has an answer for it. Every such case in Test262 declares `SyntaxError`, which
                // is also the only type this runner can attribute: a requested module that does
                // not parse, or a name no module provides, is a SyntaxError and nothing else.
                Some(negative) if negative.type_name == "SyntaxError" => Outcome::Pass,
                Some(negative) => {
                    diagnostics.push(Diagnostic::new(
                        DiagnosticPhase::Harness,
                        format!(
                            "linking failed, but the expected error type `{}` is not verifiable",
                            negative.type_name
                        ),
                        None,
                    ));
                    Outcome::Unsupported
                }
                // Otherwise the graph was supposed to link, so a refusal is a wrong answer unless
                // it is this engine admitting a gap -- and a compiler rejection that the linker
                // cannot attribute to the specification is a gap when a negative case is waiting
                // on it, because counting it would claim a check the runner never made.
                None => match failure {
                    LinkFailure::Unsupported(_) | LinkFailure::OutOfReach(_) => {
                        Outcome::Unsupported
                    }
                    LinkFailure::Compile(_) if negative.is_some() => Outcome::Unsupported,
                    _ => Outcome::Fail,
                },
            };
            return CaseResult::new(outcome, diagnostics, String::new(), started);
        }
    };

    let Some(negative) = negative else {
        return finish_run(outcome, host, is_async, diagnostics, started);
    };
    if matches!(negative.phase, Phase::Resolution) {
        diagnostics.push(Diagnostic::new(
            DiagnosticPhase::Harness,
            "expected module linking to fail for a resolution-negative case",
            None,
        ));
        return CaseResult::new(Outcome::Fail, diagnostics, host.into_stdout(), started);
    }
    match outcome {
        Ok(()) => {
            diagnostics.push(Diagnostic::new(
                DiagnosticPhase::Runtime,
                "expected a runtime exception for a runtime-negative case",
                None,
            ));
            CaseResult::new(Outcome::Fail, diagnostics, host.into_stdout(), started)
        }
        Err(error) => {
            diagnostics.push(runtime_diagnostic(&error));
            // Same reasoning as the script path: a thrown value here has no JavaScript
            // constructor identity yet, so the declared type cannot be checked and claiming a
            // pass would assert something the runner never verified.
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

/// Why a module case never became a runnable graph.
///
/// The graph reports one error type for every refusal, but a conformance verdict turns on which
/// refusal it was: a dependency that does not parse is the SyntaxError a resolution-negative case
/// is asking for, while a dependency this compiler cannot lower yet says nothing about the test.
enum LinkFailure {
    /// A requested module is not a module. That is a SyntaxError raised while the graph is being
    /// assembled, which is exactly what `phase: resolution` asserts.
    Syntax(String),
    /// A requested module uses something Kiln cannot compile yet.
    Unsupported(String),
    /// Kiln's compiler rejected a requested module for a reason it does not name as a gap. The
    /// specification may well agree -- a duplicated export is an early SyntaxError -- but the
    /// runner cannot tell that from a compiler bug, so this never earns a negative-test pass.
    Compile(String),
    /// The linker rejected the graph: a name no module provides, an unresolvable specifier.
    Link(String),
    /// The linker refused something the specification allows, so the case is out of reach without
    /// the refusal saying anything about it.
    OutOfReach(String),
    /// The runner or the corpus is at fault: an unreadable entry, an internal inconsistency.
    Broken(String),
}

impl LinkFailure {
    fn message(&self) -> &str {
        match self {
            Self::Syntax(message)
            | Self::Unsupported(message)
            | Self::Compile(message)
            | Self::Link(message)
            | Self::OutOfReach(message)
            | Self::Broken(message) => message,
        }
    }
}

/// Link a module case against its sibling fixtures and run the whole graph.
///
/// `Ok` means every module was linked and evaluated; the inner result is the entry's own
/// execution outcome, judged exactly like a script's.
fn link_and_execute(
    case: &TestCase,
    entry_program: ModuleProgram,
) -> Result<(Result<(), ExecutionError>, CaptureHost), LinkFailure> {
    let graph = build_module_graph(case, entry_program)?;
    let mut host = CaptureHost::new();
    match graph.execute(&mut host, conformance_limits(case)) {
        Ok(_) => Ok((Ok(()), host)),
        Err(ModuleGraphError::Execution { error, .. }) => Ok((Err(error), host)),
        Err(error) => Err(classify_graph_error(&error)),
    }
}

fn build_module_graph(
    case: &TestCase,
    entry_program: ModuleProgram,
) -> Result<ModuleGraph, LinkFailure> {
    let entry = case.path.canonicalize().map_err(|error| {
        LinkFailure::Broken(format!(
            "could not open module entry {}: {error}",
            case.path.display()
        ))
    })?;
    // Test262 modules import their fixtures as siblings, and nothing in the corpus reaches above
    // its own directory. The test's directory is therefore the smallest root that resolves every
    // specifier, which is how a corpus living outside any Kiln project satisfies the resolver's
    // containment rule -- narrowing the root rather than relaxing the rule.
    let root = entry.parent().ok_or_else(|| {
        LinkFailure::Broken(format!(
            "module entry has no directory: {}",
            entry.display()
        ))
    })?;

    // A provider returns one error type, but the verdict depends on which kind of refusal it was,
    // so the dependency's own reason is recorded as it happens and preferred over the flattened
    // error the graph hands back.
    let dependency_failure: Rc<RefCell<Option<LinkFailure>>> = Rc::new(RefCell::new(None));
    let recorded = Rc::clone(&dependency_failure);
    let mut entry_program = Some(entry_program);
    let entry_path = entry.clone();
    let result = ModuleGraph::build_with_module_provider(&entry, root, move |path: &Path| {
        if path == entry_path {
            // The entry's source is not the file on disk: the runner prepends the strictness
            // directive and the assertion harness, and the program compiled from that assembled
            // source is the one the test is about. Only dependencies are read from the corpus.
            return entry_program
                .take()
                .ok_or_else(|| ModuleGraphError::InternalMissingModule {
                    path: path.to_path_buf(),
                });
        }
        compile_dependency(path, &recorded)
    });

    result.map_err(|error| {
        dependency_failure
            .borrow_mut()
            .take()
            .unwrap_or_else(|| classify_graph_error(&error))
    })
}

/// Compile one imported fixture, straight from disk and with no harness of its own.
///
/// A fixture is not a test: it carries no frontmatter, is never wrapped in a strictness directive,
/// and is always a module, because only a module can be imported.
fn compile_dependency(
    path: &Path,
    recorded: &Rc<RefCell<Option<LinkFailure>>>,
) -> Result<ModuleProgram, ModuleGraphError> {
    let fail = |failure: LinkFailure| -> ModuleGraphError {
        let message = failure.message().to_owned();
        *recorded.borrow_mut() = Some(failure);
        ModuleGraphError::Source {
            path: path.to_path_buf(),
            message,
        }
    };
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            return Err(fail(LinkFailure::Broken(format!(
                "could not read imported module {}: {error}",
                path.display()
            ))));
        }
    };
    let Some(name) = path.to_str() else {
        return Err(fail(LinkFailure::Broken(format!(
            "imported module path is not valid UTF-8: {}",
            path.display()
        ))));
    };
    let hir = parse_with_goal(name, &source, SourceGoal::Module);
    if hir.panicked || !hir.diagnostics.is_empty() {
        let detail = hir.diagnostics.first().map_or(
            "frontend parser could not recover a program",
            |diagnostic| diagnostic.message.as_str(),
        );
        return Err(fail(LinkFailure::Syntax(format!(
            "imported module {} is not a valid module: {detail}",
            path.display()
        ))));
    }
    kiln_ir::compile_module(&hir).map_err(|diagnostics| {
        let detail = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        let message = format!(
            "could not compile imported module {}: {detail}",
            path.display()
        );
        let unsupported = diagnostics
            .iter()
            .all(|diagnostic| is_intentionally_unsupported(&diagnostic.message));
        fail(if unsupported {
            LinkFailure::Unsupported(message)
        } else {
            LinkFailure::Compile(message)
        })
    })
}

/// Read one graph refusal as a conformance verdict.
fn classify_graph_error(error: &ModuleGraphError) -> LinkFailure {
    let message = error.to_string();
    match error {
        // Test262 imports non-JavaScript modules -- JSON, text, raw bytes -- through import
        // attributes, and asks the host to resolve specifiers like `<module source>` that name no
        // file at all. Kiln resolves JavaScript files and nothing else, so refusing these says
        // what this runner cannot host rather than what the test got wrong.
        ModuleGraphError::Resolve {
            error: ResolveError::UnsupportedExtension { .. } | ResolveError::InvalidSpecifier { .. },
            ..
        } => LinkFailure::OutOfReach(message),
        // A missing or ambiguous export, a specifier that names nothing, a duplicated export: the
        // specification calls each of these a SyntaxError at link time.
        ModuleGraphError::MissingExport { .. }
        | ModuleGraphError::AmbiguousExport { .. }
        | ModuleGraphError::DuplicateExport { .. }
        | ModuleGraphError::UnresolvedImport { .. }
        | ModuleGraphError::Resolve { .. } => LinkFailure::Link(message),
        // A cycle is legal JavaScript that this graph deliberately does not accept, so refusing it
        // is a statement about the linker rather than about the test.
        ModuleGraphError::Cycle { .. } => LinkFailure::OutOfReach(message),
        ModuleGraphError::Compile { diagnostics, .. } => {
            if diagnostics
                .iter()
                .all(|diagnostic| is_intentionally_unsupported(&diagnostic.message))
            {
                LinkFailure::Unsupported(message)
            } else {
                LinkFailure::Compile(message)
            }
        }
        _ => LinkFailure::Broken(message),
    }
}

/// Turn a finished run into a verdict.
///
/// Shared by the script and module paths: they differ in how the program is compiled and
/// invoked, and in nothing at all about how the result is judged.
fn finish_run(
    outcome: Result<(), ExecutionError>,
    host: CaptureHost,
    is_async: bool,
    mut diagnostics: Vec<Diagnostic>,
    started: Instant,
) -> CaseResult {
    match outcome {
        Ok(()) => {
            let stdout = host.into_stdout();
            if !is_async {
                return CaseResult::new(Outcome::Pass, diagnostics, stdout, started);
            }
            let (outcome, reason) = async_outcome(&stdout);
            if let Some(reason) = reason {
                diagnostics.push(Diagnostic::new(DiagnosticPhase::Runtime, reason, None));
            }
            CaseResult::new(outcome, diagnostics, stdout, started)
        }
        Err(error) => {
            let message = error.to_string();
            diagnostics.push(runtime_diagnostic(&error));
            // A missing host facility or a missing intrinsic is a gap in this implementation,
            // not a wrong answer from the language, and must not be counted as a failure.
            let outcome = if message.contains(HOST_CAPABILITY_MARKER) {
                Outcome::Unsupported
            } else if let Some(name) = missing_intrinsic(&message) {
                diagnostics.push(Diagnostic::new(
                    DiagnosticPhase::Runtime,
                    format!("`{name}` is not implemented"),
                    None,
                ));
                Outcome::Unsupported
            } else {
                Outcome::Fail
            };
            CaseResult::new(outcome, diagnostics, host.into_stdout(), started)
        }
    }
}

/// A compilation that failed: unsupported when every diagnostic is a known gap, a failure when
/// any of them is the compiler rejecting something it ought to accept.
fn compile_failure(
    compile_diagnostics: Vec<kiln_ir::CompileDiagnostic>,
    mut diagnostics: Vec<Diagnostic>,
    prefix_len: usize,
    started: Instant,
) -> CaseResult {
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
    CaseResult::new(
        if unsupported {
            Outcome::Unsupported
        } else {
            Outcome::Fail
        },
        diagnostics,
        String::new(),
        started,
    )
}

fn runtime_diagnostic(error: &ExecutionError) -> Diagnostic {
    Diagnostic::new(DiagnosticPhase::Runtime, error.to_string(), None)
}

/// The step budget a case that builds Unicode strings needs.
///
/// `harness/regExpUtils.js`'s `buildString` walks code points one at a time -- 1,114,112 of them
/// for a property that matches everything -- at roughly thirty steps each. That is honest work,
/// not a loop, and it is the one thing in the corpus the default budget cannot pay for.
const UNICODE_STRING_BUILDER_STEPS: u64 = 60_000_000;

/// Execution limits for a Test262 case.
///
/// The default for everything except the cases that include `regExpUtils.js`. Raising the budget
/// across the board was tried, on the theory that the assertion harness spends a share of it
/// before the test starts; the same cases exhausted the larger budget, fifty times more slowly,
/// which said they were not short of steps but looping. A step limit a passing test never
/// approaches is doing its job, and enlarging it only makes a real hang cost more wall-clock --
/// so the raise is spent on the one harness file that provably needs it and nowhere else.
fn conformance_limits(case: &TestCase) -> ExecutionLimits {
    let default = ExecutionLimits::default();
    if case_includes(case)
        .iter()
        .any(|include| include == "regExpUtils.js")
    {
        return ExecutionLimits {
            max_steps: UNICODE_STRING_BUILDER_STEPS,
            ..default
        };
    }
    default
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

/// Judge a `phase: parse` negative case from the parser's diagnostics alone.
///
/// Test262 states the contract as: parsing this source must fail.  So a diagnostic is the pass, a
/// clean parse is the failure -- the test asked for an error and Kiln produced a program -- and
/// there is no third answer to be had by running anything, because a parse-negative test is never
/// executed.  Reporting the clean-parse case as unsupported, which is what this replaced, hid a
/// real gap: an early error the parser does not raise is a wrong answer about the language, not a
/// feature the runner is missing.
fn parse_negative_outcome(
    negative: &kiln_test262::NegativeExpectation,
    module: &kiln_hir::Module,
    diagnostics: &mut Vec<Diagnostic>,
) -> Outcome {
    if module.diagnostics.is_empty() {
        // A recovered HIR or a frontend panic is not a parser diagnostic and cannot stand in for
        // one: the panic says the frontend fell over, not that it rejected the source.
        diagnostics.push(Diagnostic::new(
            DiagnosticPhase::Parse,
            if module.panicked {
                "frontend parser could not recover a program, which is not the parse error this case expects"
            } else {
                "expected a parser diagnostic for a parse-negative case"
            },
            None,
        ));
        return Outcome::Fail;
    }
    if parse_error_type_matches(negative, &module.diagnostics) {
        return Outcome::Pass;
    }
    diagnostics.push(Diagnostic::new(
        DiagnosticPhase::Parse,
        format!(
            "parser diagnostics could not verify expected error type `{}`",
            negative.type_name
        ),
        None,
    ));
    Outcome::Unsupported
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

/// The intrinsic a runtime error says is missing, when that is what it says.
///
/// A test that reaches for `DataView` and is told it is not defined has not found a wrong answer;
/// it has found a built-in Kiln does not implement yet. ADR 0017 makes every unresolved identifier
/// a runtime throw, so a missing intrinsic and a genuine scoping bug arrive in the same shape, and
/// the only thing separating them is what the name looks like: an intrinsic is capitalised and
/// unbound, while a test's own variables are declared in its source.
///
/// Misreading a real bug as a missing intrinsic costs a Fail that becomes an Unsupported, which is
/// visible and never counted as a pass — the invariant ADR 0008 exists to protect. Misreading it
/// the other way buries twelve thousand missing-feature reports among the real failures, which is
/// what it did before this existed.
fn missing_intrinsic(message: &str) -> Option<&str> {
    let rest = message.strip_prefix("uncaught ReferenceError: ")?;
    let name = rest.split(" is not defined").next()?;
    let mut characters = name.chars();
    let first = characters.next()?;
    let intrinsic_shaped = first.is_ascii_uppercase()
        && characters.all(|character| character.is_alphanumeric() || character == '_');
    intrinsic_shaped.then_some(name)
}

pub(crate) fn is_intentionally_unsupported(message: &str) -> bool {
    message.starts_with("unsupported statement:")
        || message.starts_with("unsupported expression:")
        || message.starts_with("unsupported member access")
        || message.starts_with("only console.log(")
        || message.starts_with("module dependency `")
        || message.starts_with("re-export from `")
}

fn unsupported_case_requirement(case: &TestCase, harness: Option<&Harness>) -> Option<String> {
    // A `raw` test is defined as running with no harness at all, so it needs nothing here.
    if has_flag(case, "raw") {
        return None;
    }
    match harness {
        // Every non-`raw` test calls into `assert.js`, so without a harness the only honest
        // outcome is a skip that names what is missing.
        None => Some("Test262 assertion harness was not found beside this tree".to_owned()),
        // A harness that is present but missing a file the test asked for is a broken checkout,
        // not an unimplemented language feature, and must not be reported as one.
        Some(harness) => harness.prelude_for(&case_includes(case)).err(),
    }
}

/// The harness files a case needs, which for an `async` test includes `$DONE`'s definition.
///
/// Test262 requires `doneprintHandle.js` for every async test but does not make each test list it,
/// so the runner supplies it. It is added rather than substituted: an async test may have includes
/// of its own.
fn case_includes(case: &TestCase) -> Vec<String> {
    let mut includes = case.metadata.includes.clone();
    if has_flag(case, "async") && !includes.iter().any(|name| name == ASYNC_INCLUDE) {
        includes.push(ASYNC_INCLUDE.to_owned());
    }
    includes
}

/// Judge an `async` case from what `$DONE` printed.
///
/// An async test signals its own result rather than merely running to completion, so finishing
/// quietly is a failure: it means the test never reached `$DONE`, which is exactly the bug the
/// protocol exists to catch.
fn async_outcome(stdout: &str) -> (Outcome, Option<String>) {
    if let Some(start) = stdout.find(ASYNC_FAILURE) {
        let message = stdout[start..].lines().next().unwrap_or(ASYNC_FAILURE);
        return (Outcome::Fail, Some(message.to_owned()));
    }
    if stdout.contains(ASYNC_COMPLETE) {
        return (Outcome::Pass, None);
    }
    (
        Outcome::Fail,
        Some("async test finished without calling $DONE".to_owned()),
    )
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

    use crate::harness::Harness;

    fn case(source: &str) -> TestCase {
        TestCase {
            path: PathBuf::from("fixture.js"),
            source: source.to_owned(),
            metadata: Metadata::default(),
        }
    }

    /// A directory holding one module case and the fixtures it imports.
    ///
    /// Linking is filesystem behaviour: the specifiers are relative, the fixtures are real files,
    /// and the containment root is derived from where the entry actually lives. Nothing about that
    /// can be exercised against an in-memory path.
    struct TempCorpus {
        root: PathBuf,
    }

    impl TempCorpus {
        fn new(label: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let ordinal = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "kiln-conformance-{label}-{}-{ordinal}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).expect("create temp corpus");
            Self { root }
        }

        fn write(&self, name: &str, source: &str) -> PathBuf {
            let path = self.root.join(name);
            std::fs::write(&path, source).expect("write module");
            path
        }

        /// A `raw`, `module`-flagged case whose entry file also exists on disk.
        ///
        /// `on_disk` is deliberately separate from `source`: the runner assembles the entry it
        /// runs, and the linker has to use that assembled program rather than re-reading the file.
        fn module_case(&self, name: &str, on_disk: &str, source: &str) -> TestCase {
            let path = self.write(name, on_disk);
            let mut case = case(source);
            case.path = path;
            case.metadata.flags.push("module".to_owned());
            case.metadata.flags.push("raw".to_owned());
            case
        }
    }

    impl Drop for TempCorpus {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_module_case_links_and_runs_its_sibling_fixtures() {
        let corpus = TempCorpus::new("link");
        corpus.write("dep_FIXTURE.js", "export const answer = 42;\n");
        let entry = corpus.module_case(
            "entry.js",
            "",
            "import { answer } from './dep_FIXTURE.js';\nif (answer !== 42) { wrong_answer(); }\n",
        );
        assert_eq!(
            run_case(&entry, RunMode::Interpreter).outcome,
            Outcome::Pass
        );
    }

    #[test]
    fn the_linked_entry_is_the_assembled_source_rather_than_the_file_on_disk() {
        // The runner prepends a strictness directive and the assertion harness before running a
        // case, so the file beside the fixtures is not what the test is. Linking the file instead
        // would silently run a different program.
        let corpus = TempCorpus::new("assembled");
        corpus.write("dep_FIXTURE.js", "export const answer = 42;\n");
        let entry = corpus.module_case(
            "entry.js",
            "import { absent } from './dep_FIXTURE.js';\n",
            "import { answer } from './dep_FIXTURE.js';\nif (answer !== 42) { wrong_answer(); }\n",
        );
        assert_eq!(
            run_case(&entry, RunMode::Interpreter).outcome,
            Outcome::Pass
        );
    }

    #[test]
    fn a_dependency_that_cannot_be_resolved_is_a_failure_not_a_pass() {
        let corpus = TempCorpus::new("missing-dep");
        let entry = corpus.module_case(
            "entry.js",
            "",
            "import { answer } from './absent_FIXTURE.js';\n",
        );
        let result = run_case(&entry, RunMode::Interpreter);
        assert_eq!(result.outcome, Outcome::Fail);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("absent_FIXTURE.js"))
        );
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
    fn a_parse_negative_case_is_judged_by_the_parser_in_interpreter_mode() {
        // The interpreter runner used to call every parse-negative case unsupported, which put
        // thousands of tests it can actually decide outside the conformance measurement. Kiln has
        // a parser; a `phase: parse` case is exactly the question a parser answers.
        let mut rejected = case("const = ;");
        rejected.metadata.negative = Some(NegativeExpectation {
            phase: Phase::Parse,
            type_name: "SyntaxError".to_owned(),
        });
        assert_eq!(
            run_case(&rejected, RunMode::Interpreter).outcome,
            Outcome::Pass,
            "a parse-negative case needs no harness: it is never executed"
        );

        // The half that matters for honesty. An early error Kiln's parser does not raise is a
        // wrong answer about the language, and reporting it as unsupported hid it among the
        // features the runner has genuinely not reached yet.
        let mut accepted = case("const answer = 42;");
        accepted.metadata.negative = Some(NegativeExpectation {
            phase: Phase::Parse,
            type_name: "SyntaxError".to_owned(),
        });
        let result = run_case(&accepted, RunMode::Interpreter);
        assert_eq!(result.outcome, Outcome::Fail);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("expected a parser diagnostic"))
        );

        // A declared type other than `SyntaxError` cannot be checked against an Oxc diagnostic,
        // so it stays visible as unsupported rather than being counted either way.
        let mut other_type = case("const = ;");
        other_type.metadata.negative = Some(NegativeExpectation {
            phase: Phase::Parse,
            type_name: "TypeError".to_owned(),
        });
        assert_eq!(
            run_case(&other_type, RunMode::Interpreter).outcome,
            Outcome::Unsupported
        );
    }

    #[test]
    fn a_module_parse_negative_case_does_not_wait_for_a_linker() {
        // Parse-phase errors are found while parsing the module, before anything is linked, so
        // the missing linker is no reason to withhold a verdict here.
        let mut module = case("import { a } from;");
        module.metadata.flags.push("module".to_owned());
        module.metadata.negative = Some(NegativeExpectation {
            phase: Phase::Parse,
            type_name: "SyntaxError".to_owned(),
        });
        assert_eq!(
            run_case(&module, RunMode::Interpreter).outcome,
            Outcome::Pass
        );
    }

    #[test]
    fn a_resolution_negative_case_is_judged_by_the_linker() {
        // Resolution asks whether linking a module graph fails, and a name no module provides is
        // the SyntaxError it is asking for.
        let corpus = TempCorpus::new("resolution");
        corpus.write("dep_FIXTURE.js", "export const answer = 42;\n");
        let mut entry = corpus.module_case(
            "entry.js",
            "",
            "import { missing } from './dep_FIXTURE.js';\n",
        );
        entry.metadata.negative = Some(NegativeExpectation {
            phase: Phase::Resolution,
            type_name: "SyntaxError".to_owned(),
        });
        assert_eq!(
            run_case(&entry, RunMode::Interpreter).outcome,
            Outcome::Pass
        );
    }

    #[test]
    fn a_resolution_negative_case_that_links_cleanly_is_a_failure() {
        // The assertion is that linking fails. A graph that links is the test's answer being
        // wrong, and must never be reported as a pass for having run.
        let corpus = TempCorpus::new("resolution-links");
        corpus.write("dep_FIXTURE.js", "export const answer = 42;\n");
        let mut entry = corpus.module_case(
            "entry.js",
            "",
            "import { answer } from './dep_FIXTURE.js';\n",
        );
        entry.metadata.negative = Some(NegativeExpectation {
            phase: Phase::Resolution,
            type_name: "SyntaxError".to_owned(),
        });
        assert_eq!(
            run_case(&entry, RunMode::Interpreter).outcome,
            Outcome::Fail
        );
    }

    #[test]
    fn a_dependency_that_is_not_a_module_is_the_resolution_phase_syntax_error() {
        let corpus = TempCorpus::new("resolution-syntax");
        corpus.write("dep_FIXTURE.js", "export const = ;\n");
        let mut entry = corpus.module_case(
            "entry.js",
            "",
            "import { answer } from './dep_FIXTURE.js';\n",
        );
        entry.metadata.negative = Some(NegativeExpectation {
            phase: Phase::Resolution,
            type_name: "SyntaxError".to_owned(),
        });
        assert_eq!(
            run_case(&entry, RunMode::Interpreter).outcome,
            Outcome::Pass
        );
    }

    #[test]
    fn unsupported_hir_is_not_a_negative_runtime_pass() {
        // Whatever construct stands in here has to be one the frontend still refuses, so this
        // test has moved four times as coverage grew -- `function f() {}`, `switch (0) {}`,
        // `async function f() {}` and `#private` all stopped refusing. `with` should be the last
        // move: it is forbidden in strict mode, modules are always strict, and implementing it
        // would be a step backwards. That churn was the signal working, though: a refused
        // construct must never be scored as a negative test's expected failure.
        let mut negative = case("with (this) {}\n");
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
    fn a_case_without_a_harness_is_skipped_rather_than_failed() {
        let mut flagged = case("console.log('must not run');");
        flagged.metadata.flags.push("async".to_owned());
        let result = run_case(&flagged, RunMode::Interpreter);
        assert_eq!(result.outcome, Outcome::Skipped);
        assert!(result.stdout.is_empty());
        assert!(result.diagnostics[0].message.contains("harness"));
    }

    /// A harness holding the real `$DONE`, so the async tests below exercise the actual protocol
    /// rather than a paraphrase of it.
    fn async_harness(label: &str) -> Harness {
        use std::time::{SystemTime, UNIX_EPOCH};
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("kiln-async-{label}-{stamp}"));
        std::fs::create_dir_all(&root).expect("create");
        std::fs::write(root.join("assert.js"), "").expect("assert");
        std::fs::write(root.join("sta.js"), "").expect("sta");
        std::fs::write(
            root.join("doneprintHandle.js"),
            r#"function $DONE(error) {
  if (error) {
    print('Test262:AsyncTestFailure:Test262Error: ' + String(error));
  } else {
    print('Test262:AsyncTestComplete');
  }
}
"#,
        )
        .expect("done");
        Harness::load(&root).expect("load")
    }

    fn async_case(source: &str) -> TestCase {
        let mut built = case(source);
        built.metadata.flags.push("async".to_owned());
        built
    }

    #[test]
    fn an_async_case_passes_only_when_done_says_so() {
        let harness = async_harness("pass");
        let result = run_case_with_harness(
            &async_case("$DONE();"),
            RunMode::Interpreter,
            Some(&harness),
        );
        assert_eq!(result.outcome, Outcome::Pass);
    }

    #[test]
    fn an_async_case_that_never_calls_done_fails() {
        // The protocol exists to catch exactly this: a test whose asynchronous half never ran
        // finishes quietly, and treating that as success would make it permanently green.
        let harness = async_harness("silent");
        let result = run_case_with_harness(
            &async_case("var unused = 1;"),
            RunMode::Interpreter,
            Some(&harness),
        );
        assert_eq!(result.outcome, Outcome::Fail);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("without calling $DONE"))
        );
    }

    #[test]
    fn an_async_failure_is_reported_with_the_message_done_printed() {
        let harness = async_harness("fail");
        let result = run_case_with_harness(
            &async_case("$DONE('boom');"),
            RunMode::Interpreter,
            Some(&harness),
        );
        assert_eq!(result.outcome, Outcome::Fail);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("boom"))
        );
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
