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

use kiln_db::{Compiled, FileId, Interface, MemoEngine, Parsed, Project, QueryEngine, Stats};
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
        // A body that varies with the source. It was empty at first, which made every text in
        // the pool compile to identical bytecode and turned the `irs` comparison in `Observed`
        // into a comparison of a constant. One statement per line, carrying that line's length,
        // is enough to make the IR discriminate without pretending this is a real lowering.
        body: text
            .lines()
            .enumerate()
            .map(|(line, source_line)| HirStatement {
                kind: HirStatementKind::Expression(HirExpression {
                    kind: HirExpressionKind::Literal(Literal::Number(source_line.len() as u64)),
                    span: Span::new(line as u32, line as u32 + 1),
                }),
                span: Span::new(line as u32, line as u32 + 1),
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
    }
}

use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

/// Deliberately tiny pools. Collisions and reversions are the interesting inputs - re-writing a
/// file's previous text, or a body edit that leaves the interface alone - and a wide pool makes
/// them vanishingly rare.
const PATHS: [&str; 4] = ["a.js", "b.js", "c.js", "d.js"];

const TEXTS: [&str; 7] = [
    "",
    "const hidden = 1;\n",
    "export const alpha = 1;\n",
    // Same interface as the line above, different implementation: the edit that must cut off.
    "export const alpha = 2;\n",
    "export const alpha = 1;\nexport const beta = 2;\n",
    // An interface is a set of names, so this is the same interface as the line above.
    "export const beta = 2;\nexport const alpha = 1;\n",
    "export function alpha() {}\nexport function alpha() {}\n",
];

/// One step of a generated history. Indices are reduced modulo the live state when the step runs,
/// so a shrunk sequence stays executable instead of addressing a file that no longer exists.
#[derive(Debug, Clone, Copy)]
enum Op {
    Intern(usize),
    SetSource(usize, usize),
    Parse(usize),
    Interface(usize),
    Project,
}

fn histories() -> impl Strategy<Value = Vec<Op>> {
    let op = prop_oneof![
        2 => (0..PATHS.len()).prop_map(Op::Intern),
        6 => (0..PATHS.len(), 0..TEXTS.len()).prop_map(|(file, text)| Op::SetSource(file, text)),
        2 => (0..PATHS.len()).prop_map(Op::Parse),
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
    let mut interfaces = Vec::with_capacity(files);
    for index in 0..files {
        let file = FileId(index as u32);
        sources.push(db.source(file).unwrap_or_default().to_string());
        parses.push(db.parse(file));
        irs.push(db.ir(file));
        interfaces.push(db.interface(file));
    }

    Observed {
        sources,
        parses,
        irs,
        interfaces,
        project: project.unwrap_or_else(|| db.project()),
    }
}

/// The oracle: an engine fed the current inputs in interning order and nothing else, so its
/// answers cannot depend on any history.
///
/// `None` means interned but never written, which the engine distinguishes from written-empty:
/// a first write is a real edit even when the text is `""`. Replaying it as `set_source(_, "")`
/// would make the oracle disagree with the engine about what has happened, so the distinction
/// has to be carried here too.
fn from_scratch(inputs: &[(String, Option<String>)]) -> MemoEngine {
    let mut db = MemoEngine::new(oracle_frontend);
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
        Op::Interface(slot) if !inputs.is_empty() => {
            db.interface(FileId((slot % inputs.len()) as u32));
        }
        Op::Project => {
            db.project();
        }
        // A file-addressed step before anything is interned has nothing to address.
        Op::SetSource(..) | Op::Parse(_) | Op::Interface(_) => {}
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
    let mut db = MemoEngine::new(oracle_frontend);
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
        let mut db = MemoEngine::new(oracle_frontend);
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
        let mut db = MemoEngine::new(oracle_frontend);
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
