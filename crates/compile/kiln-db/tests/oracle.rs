//! A differential oracle for the query engine.
//!
//! The unit tests in `engine.rs` cover the invalidation cases someone thought of. The failure
//! adr/0001 actually warns about is the one nobody thought of - "the quiet kind where the
//! compiler produces a stale answer rather than crashing" - so this suite does not enumerate
//! cases at all. It generates a random history, and after every step asserts that the memoized
//! engine agrees with an engine that has seen nothing but the current inputs. A dropped
//! dependency, a `verified_at` bumped on a path that did not really validate, an early cutoff
//! that kept `changed_at` when the value moved: all of them surface as one engine disagreeing
//! with the other, without the test having to know which mistake it is looking for.
//!
//! The invariant is stated in terms of the `QueryEngine` trait alone, so it transfers unchanged
//! to a salsa-backed implementation - which is the acceptance suite adr/0001 promises.
//!
//! `Stats` assertions are layered on top rather than being the point: a counter claim is about
//! how a particular engine reaches the answer, and only memoized-equals-fresh survives a change
//! of backend.

use kiln_db::{
    Compiled, FileId, FunctionSummary, Interface, MemoEngine, Parsed, Project, QueryEngine, Stats,
};
use kiln_hir::{
    Export, ExportName, ExportOrigin, Expression as HirExpression,
    ExpressionKind as HirExpressionKind, Item, ItemKind, Literal, Module, SourceGoal, SourceKind,
    Span, Statement as HirStatement, StatementKind as HirStatementKind,
};

/// The oracle supplies its own frontend rather than borrowing a real one.
///
/// `MemoEngine::new` takes the frontend as a parameter (ADR 0006), and the property under test
/// is a claim about memoization, not about parsing: any deterministic `(path, source) -> Module`
/// exercises it equally. A trivial one keeps the suite inside the Tier 0 budget in design.md §9
/// and means a frontend change can never make this test fail for the wrong reason.
///
/// Export extraction mirrors the engine's own fixture frontend, so that a body-only edit really
/// does leave the interface alone - which is what makes the early-cutoff assertions meaningful.
fn oracle_frontend(_path: &str, text: &str) -> Module {
    const DECLARATIONS: [&str; 6] = [
        "async function ",
        "function ",
        "const ",
        "let ",
        "var ",
        "class ",
    ];

    let mut exports = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("export ") else {
            continue;
        };
        let rest = rest.trim_start();
        let rest = DECLARATIONS
            .iter()
            .find_map(|kw| rest.strip_prefix(kw))
            .unwrap_or(rest);
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if !name.is_empty() {
            exports.push(name);
        }
    }

    Module {
        module_id: "".into(),
        // No source, deliberately. This frontend synthesizes a fixed shape and never reads
        // `text`, so its spans index into nothing and its bytecode is the same whatever the file
        // says. Handing it the text would imply a relationship that does not exist -- and would
        // break the early-cutoff tests below, which edit a file expecting the *same* program
        // back. That expectation is real for this stub and false for a real frontend, where a
        // comment-only edit now does produce a different `Program`: the bytecode is unchanged but
        // the text `Function.prototype.toString` answers with is not.
        source: "".into(),
        static_source_provenance: None,
        source_origin: None,
        source_kind: SourceKind::JavaScript,
        source_goal: SourceGoal::Module,
        strict: false,
        items: text
            .lines()
            .enumerate()
            .map(|(line, _)| Item {
                kind: ItemKind::Other,
                span: Span::new(line as u32, line as u32 + 1),
            })
            .collect(),
        // A body that varies with the source. Functions generate real Function statements so that
        // summaries discriminate and early cutoff can be tested across both functions and variables.
        body: text
            .lines()
            .enumerate()
            .map(|(line, source_line)| {
                let trimmed = source_line.trim_start();
                let stripped = trimmed
                    .strip_prefix("export ")
                    .unwrap_or(trimmed)
                    .trim_start();
                if stripped.starts_with("function ") || stripped.starts_with("async function ") {
                    let rest = stripped.strip_prefix("async ").unwrap_or(stripped);
                    let rest = rest.strip_prefix("function ").unwrap_or(rest).trim_start();
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                        .collect();
                    let name = if name.is_empty() {
                        "fn".to_string()
                    } else {
                        name
                    };

                    let mut fn_body = Vec::new();
                    if source_line.contains("throw") {
                        fn_body.push(HirStatement {
                            kind: HirStatementKind::Throw(HirExpression {
                                kind: HirExpressionKind::Literal(Literal::Number(1)),
                                span: Span::new(line as u32, line as u32 + 1),
                            }),
                            span: Span::new(line as u32, line as u32 + 1),
                        });
                    } else if source_line.contains("return \"") || source_line.contains("return '")
                    {
                        fn_body.push(HirStatement {
                            kind: HirStatementKind::Return(Some(HirExpression {
                                kind: HirExpressionKind::Literal(Literal::String("str".into())),
                                span: Span::new(line as u32, line as u32 + 1),
                            })),
                            span: Span::new(line as u32, line as u32 + 1),
                        });
                    } else if source_line.contains("return") {
                        let num = if source_line.contains("return 2") {
                            2
                        } else if source_line.contains("return 1") {
                            1
                        } else {
                            source_line.len() as u64
                        };
                        fn_body.push(HirStatement {
                            kind: HirStatementKind::Return(Some(HirExpression {
                                kind: HirExpressionKind::Literal(Literal::Number(num)),
                                span: Span::new(line as u32, line as u32 + 1),
                            })),
                            span: Span::new(line as u32, line as u32 + 1),
                        });
                    }

                    HirStatement {
                        kind: HirStatementKind::Function(Box::new(kiln_hir::Function {
                            strict: false,
                            binds_own_name: false,
                            asynchronous: stripped.starts_with("async "),
                            name: Some(name),
                            inferred_name: None,
                            arrow: false,
                            generator: false,
                            parameters: Vec::new(),
                            rest: None,
                            return_type: None,
                            body: fn_body,
                            span: Span::new(line as u32, line as u32 + 1),
                        })),
                        span: Span::new(line as u32, line as u32 + 1),
                    }
                } else {
                    HirStatement {
                        kind: HirStatementKind::Expression(HirExpression {
                            kind: HirExpressionKind::Literal(Literal::Number(
                                source_line.len() as u64
                            )),
                            span: Span::new(line as u32, line as u32 + 1),
                        }),
                        span: Span::new(line as u32, line as u32 + 1),
                    }
                }
            })
            .collect(),
        requested_modules: Vec::new(),
        imports: Vec::new(),
        exports: exports
            .into_iter()
            .map(|name| Export {
                exported: Some(ExportName::Name(name.clone())),
                origin: ExportOrigin::Local { local: name },
                type_only: false,
                span: Span::default(),
                statement_span: Span::default(),
            })
            .collect(),
        diagnostics: Vec::new(),
        panicked: false,
        node_runtime_requests: Vec::new(),
    }
}

use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

/// Deliberately tiny pools. Collisions and reversions are the interesting inputs - re-writing a
/// file's previous text, or a body edit that leaves the interface alone - and a wide pool makes
/// them vanishingly rare.
const PATHS: [&str; 4] = ["a.js", "b.js", "c.js", "d.js"];

const TEXTS: [&str; 10] = [
    "",
    "const hidden = 1;\n",
    "export const alpha = 1;\n",
    // Same interface as the line above, different implementation: the edit that must cut off.
    "export const alpha = 2;\n",
    "export const alpha = 1;\nexport const beta = 2;\n",
    // An interface is a set of names, so this is the same interface as the line above.
    "export const beta = 2;\nexport const alpha = 1;\n",
    "export function alpha() { return 1; }\n",
    // Same summary as the line above (returns number, pure, no throw): early cutoff at interface.
    "export function alpha() { return 2; }\n",
    // Different summary (returns string):
    "export function alpha() { return \"str\"; }\n",
    // Different summary (throws):
    "export function alpha() { throw 1; }\n",
];

/// One step of a generated history. Indices are reduced modulo the live state when the step runs,
/// so a shrunk sequence stays executable instead of addressing a file that no longer exists.
#[derive(Debug, Clone, Copy)]
enum Op {
    Intern(usize),
    SetSource(usize, usize),
    Parse(usize),
    Summaries(usize),
    Interface(usize),
    Project,
}

fn histories() -> impl Strategy<Value = Vec<Op>> {
    let op = prop_oneof![
        2 => (0..PATHS.len()).prop_map(Op::Intern),
        6 => (0..PATHS.len(), 0..TEXTS.len()).prop_map(|(file, text)| Op::SetSource(file, text)),
        2 => (0..PATHS.len()).prop_map(Op::Parse),
        2 => (0..PATHS.len()).prop_map(Op::Summaries),
        2 => (0..PATHS.len()).prop_map(Op::Interface),
        2 => Just(Op::Project),
    ];
    // A handful of files and tens of operations: the bugs this is aimed at need a couple of edits
    // and a stale memo, not a large project, and the suite has to stay inside Tier 0
    // (design.md §9).
    proptest::collection::vec(op, 1..24)
}

/// Everything a caller can observe, so that "the engines agree" is one comparison rather than a
/// checklist a future query could fall off.
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    sources: Vec<String>,
    parses: Vec<Parsed>,
    /// Compiled bytecode. Memoized independently of `interface`, with its own `ir_computed`
    /// counter, so it needs its own comparison: an engine that memoized `ir` wrongly while
    /// keeping `parse` and `interface` correct would satisfy every other field here.
    ///
    /// This was missing when `ir` was added to the trait, which is exactly the failure this
    /// struct's doc comment warns about. When a query joins `QueryEngine`, add it here in the
    /// same change.
    irs: Vec<Compiled>,
    summaries: Vec<Vec<FunctionSummary>>,
    interfaces: Vec<Interface>,
    project: Project,
}

/// `top_down` forces the rollup before the leaves. The two orders exercise different halves of
/// the algorithm - a validation walk that has to recurse, versus one that finds its dependencies
/// already verified - and alternating them costs nothing.
fn observe(db: &mut MemoEngine, files: usize, top_down: bool) -> Observed {
    let project = if top_down { Some(db.project()) } else { None };

    let mut sources = Vec::with_capacity(files);
    let mut parses = Vec::with_capacity(files);
    let mut irs = Vec::with_capacity(files);
    let mut summaries = Vec::with_capacity(files);
    let mut interfaces = Vec::with_capacity(files);
    for index in 0..files {
        let file = FileId(index as u32);
        sources.push(db.source(file).unwrap_or_default().to_string());
        parses.push(db.parse(file));
        irs.push(db.ir(file));
        summaries.push(db.function_summaries(file));
        interfaces.push(db.interface(file));
    }

    Observed {
        sources,
        parses,
        irs,
        summaries,
        interfaces,
        project: project.unwrap_or_else(|| db.project()),
    }
}

fn fixture_frontend(_path: &str, text: &str) -> Module {
    // `export default ...` really does export the name "default", so the bare fallthrough below
    // gets that case right without special handling.
    const DECLARATIONS: [&str; 6] = [
        "async function ",
        "function ",
        "const ",
        "let ",
        "var ",
        "class ",
    ];

    let mut exports = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("export ") else {
            continue;
        };
        let rest = rest.trim_start();
        let rest = DECLARATIONS
            .iter()
            .find_map(|kw| rest.strip_prefix(kw))
            .unwrap_or(rest);
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if !name.is_empty() {
            exports.push(name);
        }
    }

    Module {
        module_id: "".into(),
        // No source, deliberately. This frontend synthesizes a fixed shape and never reads
        // `text`, so its spans index into nothing and its bytecode is the same whatever the file
        // says. Handing it the text would imply a relationship that does not exist -- and would
        // break the early-cutoff tests below, which edit a file expecting the *same* program
        // back. That expectation is real for this stub and false for a real frontend, where a
        // comment-only edit now does produce a different `Program`: the bytecode is unchanged but
        // the text `Function.prototype.toString` answers with is not.
        source: "".into(),
        static_source_provenance: None,
        source_origin: None,
        source_kind: SourceKind::JavaScript,
        source_goal: kiln_hir::SourceGoal::Module,
        strict: false,
        items: text
            .lines()
            .enumerate()
            .map(|(line, _)| Item {
                kind: ItemKind::Other,
                span: Span::new(line as u32, line as u32 + 1),
            })
            .collect(),
        body: Vec::new(),
        requested_modules: Vec::new(),
        imports: Vec::new(),
        exports: exports
            .into_iter()
            .map(|name| Export {
                exported: Some(ExportName::Name(name.clone())),
                origin: ExportOrigin::Local { local: name },
                type_only: false,
                span: Span::default(),
                statement_span: Span::default(),
            })
            .collect(),
        diagnostics: Vec::new(),
        panicked: false,
        node_runtime_requests: Vec::new(),
    }
}

fn summary_frontend(path: &str, text: &str) -> Module {
    let mut module = fixture_frontend(path, text);
    for (idx, line) in text.lines().enumerate() {
        if line.contains("function") {
            let mut fn_body = Vec::new();
            if line.contains("throw") {
                fn_body.push(kiln_hir::Statement {
                    kind: kiln_hir::StatementKind::Throw(kiln_hir::Expression {
                        kind: kiln_hir::ExpressionKind::Literal(kiln_hir::Literal::Number(1)),
                        span: Span::default(),
                    }),
                    span: Span::default(),
                });
            } else if line.contains("return \"") || line.contains("return '") {
                fn_body.push(kiln_hir::Statement {
                    kind: kiln_hir::StatementKind::Return(Some(kiln_hir::Expression {
                        kind: kiln_hir::ExpressionKind::Literal(kiln_hir::Literal::String(
                            "str".into(),
                        )),
                        span: Span::default(),
                    })),
                    span: Span::default(),
                });
            } else if line.contains("return") {
                let num = if line.contains("return 2") { 2 } else { 1 };
                fn_body.push(kiln_hir::Statement {
                    kind: kiln_hir::StatementKind::Return(Some(kiln_hir::Expression {
                        kind: kiln_hir::ExpressionKind::Literal(kiln_hir::Literal::Number(num)),
                        span: Span::default(),
                    })),
                    span: Span::default(),
                });
            }
            module.body.push(kiln_hir::Statement {
                kind: kiln_hir::StatementKind::Function(Box::new(kiln_hir::Function {
                    strict: false,
                    binds_own_name: false,
                    asynchronous: false,
                    name: Some("f".to_owned()),
                    inferred_name: None,
                    arrow: false,
                    generator: false,
                    parameters: Vec::new(),
                    rest: None,
                    return_type: None,
                    body: fn_body,
                    span: Span::default(),
                })),
                span: Span::new(idx as u32, idx as u32 + 1),
            });
        }
    }
    module
}

#[test]
fn function_summaries_are_memoized_and_cutoff_when_unchanged() {
    let mut db =
        MemoEngine::with_summary_query(summary_frontend, kiln_backend_c::summarize_program);
    let file = db.intern("a.js");
    db.set_source(file, "function f() { return 1; }\n".to_owned());

    let first = db.function_summaries(file);
    assert_eq!(first.len(), 1);
    assert_eq!(db.stats().parse_computed, 1);
    assert_eq!(db.stats().ir_computed, 1);

    db.reset_stats();
    let second = db.function_summaries(file);
    assert_eq!(second, first);
    assert_eq!(db.stats().validated, 1);

    // Edit that changes the constant returned (1 -> 2), keeping summary identical.
    db.reset_stats();
    db.set_source(file, "function f() { return 2; }\n".to_owned());
    let third = db.function_summaries(file);
    assert_eq!(third, first);
    assert_eq!(db.stats().parse_computed, 1);
    assert_eq!(db.stats().ir_computed, 1);
    assert_eq!(db.stats().early_cutoff, 1);
}

#[test]
fn summary_changing_body_edit_propagates_to_interface_and_project() {
    let mut db =
        MemoEngine::with_summary_query(summary_frontend, kiln_backend_c::summarize_program);
    let file = db.intern("a.js");
    db.set_source(file, "export function f() { return 1; }\n".to_owned());
    let before_project = db.project();
    let before_interface = db.interface(file);

    // Edit return type from Number to String -> changes summary and interface_hash.
    db.reset_stats();
    db.set_source(file, "export function f() { return \"str\"; }\n".to_owned());
    let after_project = db.project();
    let after_interface = db.interface(file);

    assert_ne!(
        before_interface.summaries_digest,
        after_interface.summaries_digest
    );
    assert_ne!(
        before_interface.interface_hash,
        after_interface.interface_hash
    );
    assert_ne!(
        before_project.identity_digest,
        after_project.identity_digest
    );
    assert_eq!(db.stats().project_computed, 1);
}

#[test]
fn summary_preserving_body_edit_cuts_off_at_interface() {
    let mut db =
        MemoEngine::with_summary_query(summary_frontend, kiln_backend_c::summarize_program);
    let file = db.intern("a.js");
    db.set_source(file, "export function f() { return 1; }\n".to_owned());
    let before_project = db.project();
    let before_interface = db.interface(file);

    // Edit body 1 -> 2 preserves summary (both return TypeGuess::Number).
    db.reset_stats();
    db.set_source(file, "export function f() { return 2; }\n".to_owned());
    let after_project = db.project();
    let after_interface = db.interface(file);

    assert_eq!(before_interface, after_interface);
    assert_eq!(before_project, after_project);
    assert_eq!(db.stats().project_computed, 0);
    // ir recomputes (different bytecode), summaries recompute (same summary -> cutoff),
    // interface recomputes (same interface -> cutoff).
    assert_eq!(db.stats().early_cutoff, 2);
}

/// The oracle: an engine fed the current inputs in interning order and nothing else, so its
/// answers cannot depend on any history.
///
/// `None` means interned but never written, which the engine distinguishes from written-empty:
/// a first write is a real edit even when the text is `""`. Replaying it as `set_source(_, "")`
/// would make the oracle disagree with the engine about what has happened, so the distinction
/// has to be carried here too.
fn from_scratch(inputs: &[(String, Option<String>)]) -> MemoEngine {
    let mut db = MemoEngine::with_summary_query(oracle_frontend, kiln_backend_c::summarize_program);
    for (path, text) in inputs {
        let file = db.intern(path);
        if let Some(text) = text {
            db.set_source(file, text.clone());
        }
    }
    db
}

/// Applies one step to the engine and to the shadow inputs the oracle is rebuilt from.
fn apply(db: &mut MemoEngine, inputs: &mut Vec<(String, Option<String>)>, op: Op) {
    match op {
        Op::Intern(slot) => {
            let path = PATHS[slot % PATHS.len()];
            if !inputs.iter().any(|(known, _)| known == path) {
                inputs.push((path.to_string(), None));
            }
            db.intern(path);
        }
        Op::SetSource(slot, text) if !inputs.is_empty() => {
            let slot = slot % inputs.len();
            let text = TEXTS[text % TEXTS.len()];
            inputs[slot].1 = Some(text.to_string());
            db.set_source(FileId(slot as u32), text.to_string());
        }
        Op::Parse(slot) if !inputs.is_empty() => {
            db.parse(FileId((slot % inputs.len()) as u32));
        }
        Op::Summaries(slot) if !inputs.is_empty() => {
            db.function_summaries(FileId((slot % inputs.len()) as u32));
        }
        Op::Interface(slot) if !inputs.is_empty() => {
            db.interface(FileId((slot % inputs.len()) as u32));
        }
        Op::Project => {
            db.project();
        }
        // A file-addressed step before anything is interned has nothing to address.
        Op::SetSource(..) | Op::Parse(_) | Op::Summaries(_) | Op::Interface(_) => {}
    }
}

/// Re-sending state the engine already holds must cost nothing at all - not one parse per file.
/// That is what makes a stateless client safe to point at a warm daemon, and it is a property of
/// the inputs rather than of the cache, so it holds at every point in every history.
fn identical_rewrite_is_free(
    db: &mut MemoEngine,
    inputs: &[(String, Option<String>)],
) -> Result<(), TestCaseError> {
    let revision = db.revision();
    db.reset_stats();
    // Only files that have actually been written: re-sending `""` to a file that was interned
    // but never written is a first write, not a rewrite, and is correctly an edit.
    for (index, (_, text)) in inputs.iter().enumerate() {
        if let Some(text) = text {
            db.set_source(FileId(index as u32), text.clone());
        }
    }

    prop_assert_eq!(
        db.stats(),
        Stats {
            revision: revision.0,
            ..Stats::default()
        },
        "a byte-identical write is not an edit and must move no counter"
    );
    prop_assert_eq!(db.revision(), revision);
    Ok(())
}

/// Guards the guard: a field in [`Observed`] is only worth comparing if it can actually differ.
///
/// If `ir` were constant across the text pool - as it was when this frontend produced an empty
/// body - then comparing it would pass unconditionally and the coverage would be an illusion.
/// This fails loudly instead of quietly weakening the oracle.
#[test]
fn the_text_pool_produces_distinguishable_ir() {
    let mut db = MemoEngine::with_summary_query(oracle_frontend, kiln_backend_c::summarize_program);
    let file = db.intern("a.js");

    let mut distinct: Vec<Compiled> = Vec::new();
    for text in TEXTS {
        db.set_source(file, text.to_string());
        let compiled = db.ir(file);
        if !distinct.contains(&compiled) {
            distinct.push(compiled);
        }
    }

    assert!(
        distinct.len() > 1,
        "every text in the pool compiled to the same IR, so comparing it proves nothing"
    );
}

#[test]
fn the_text_pool_produces_distinguishable_summaries() {
    let mut db = MemoEngine::with_summary_query(oracle_frontend, kiln_backend_c::summarize_program);
    let file = db.intern("a.js");

    let mut distinct: Vec<Vec<FunctionSummary>> = Vec::new();
    for text in TEXTS {
        db.set_source(file, text.to_string());
        let summaries = db.function_summaries(file);
        if !distinct.contains(&summaries) {
            distinct.push(summaries);
        }
    }

    assert!(
        distinct.len() > 1,
        "every text in the pool produced the same function summaries, so comparing them proves nothing"
    );
}

proptest! {
    // Enough cases to find a real bug, few enough to stay in the Tier 0 budget (design.md §9).
    //
    // Failure persistence is off because the default looks for a `lib.rs` beside the failing test
    // to site its regression file next to, does not find one from an integration test, and prints
    // a warning instead of the failure. A shrunk history is a handful of `Op`s that the panic
    // already prints in full, so a reproduction is a paste rather than a file.
    #![proptest_config(ProptestConfig {
        cases: 64,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Memoized equals from-scratch, after every single step.
    #[test]
    fn memoized_agrees_with_a_fresh_engine_at_every_step(ops in histories()) {
        let mut db = MemoEngine::with_summary_query(oracle_frontend, kiln_backend_c::summarize_program);
        let mut inputs: Vec<(String, Option<String>)> = Vec::new();
        let mut warm = false;

        for (step, op) in ops.iter().enumerate() {
            let interfaces_before: Vec<Interface> = (0..inputs.len())
                .map(|index| db.interface(FileId(index as u32)))
                .collect();
            let files_before = inputs.len();
            let active_before: Vec<bool> = inputs
                .iter()
                .map(|(_, text)| text.is_some())
                .collect();
            db.reset_stats();

            apply(&mut db, &mut inputs, *op);

            let observed = observe(&mut db, inputs.len(), step % 2 == 0);
            let project_computed = db.stats().project_computed;

            let mut oracle = from_scratch(&inputs);
            let expected = observe(&mut oracle, inputs.len(), step % 2 == 1);
            prop_assert_eq!(
                &observed,
                &expected,
                "step {} ({:?}) disagrees with a fresh engine; history: {:?}",
                step,
                op,
                ops
            );

            // The rollup reads the file set, every interface, and runtime linkage, so if none
            // moved, no amount of editing underneath them may reach it. Activating an
            // interned-but-unset path changes the file set even if its interface is empty.
            let active_after: Vec<bool> = inputs
                .iter()
                .map(|(_, text)| text.is_some())
                .collect();
            if warm
                && inputs.len() == files_before
                && active_after == active_before
                && observed.interfaces == interfaces_before
            {
                prop_assert_eq!(
                    project_computed,
                    0,
                    "step {} ({:?}) recomputed the rollup with no interface changed; history: {:?}",
                    step,
                    op,
                    ops
                );
            }
            warm = true;

            identical_rewrite_is_free(&mut db, &inputs)?;
        }
    }

    /// The same histories with the per-step comparison removed.
    ///
    /// Comparing after every step leaves the engine fully verified, which is the state in which a
    /// stale memo is hardest to catch: a history whose edits pile up before anything reads them
    /// never occurs. Here only the generated queries run, so several edits land between reads and
    /// a memo has room to go quietly wrong.
    #[test]
    fn memoized_agrees_with_a_fresh_engine_after_a_batch(ops in histories()) {
        let mut db = MemoEngine::with_summary_query(oracle_frontend, kiln_backend_c::summarize_program);
        let mut inputs: Vec<(String, Option<String>)> = Vec::new();
        for op in &ops {
            apply(&mut db, &mut inputs, *op);
        }

        let observed = observe(&mut db, inputs.len(), true);
        let mut oracle = from_scratch(&inputs);
        let expected = observe(&mut oracle, inputs.len(), false);

        prop_assert_eq!(&observed, &expected, "batched history diverged: {:?}", ops);
    }
}
