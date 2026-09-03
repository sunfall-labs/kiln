//! The memo oracle: a memoized query engine must answer what a from-scratch one answers.
//!
//! The property, the frontend, the text pool and the reasoning are `crates/kiln-db/tests/oracle.rs`'s.
//! That file is a proptest suite and stays exactly as it is; this is the same property behind a
//! coverage-guided generator, which is the one thing proptest cannot give it. Random histories
//! find bugs in proportion to how often they happen to hit a shape; a coverage-guided engine
//! keeps the histories that reached a new branch of the invalidation walk and mutates those.
//! `MemoEngine`'s validation walk is exactly the kind of code where that difference tells --
//! most of its branches need a *particular* sequence of edits and reads to reach at all.
//!
//! ADR 0001's own words for why the property is stated this way: the failure it warns about is
//! "the quiet kind where the compiler produces a stale answer rather than crashing". So this does
//! not enumerate invalidation cases. It generates a history and asserts, after every step, that
//! the engine agrees with one that has seen nothing but the current inputs. A dropped dependency,
//! a `verified_at` bumped on a path that did not really validate, an early cutoff that kept
//! `changed_at` when the value moved -- all surface as disagreement, without the test having to
//! know which mistake it is looking for.

use arbitrary::Arbitrary;
use kiln_db::{Compiled, FileId, Interface, MemoEngine, Parsed, Project, QueryEngine, Stats};
use kiln_hir::{
    Export, ExportName, ExportOrigin, Expression as HirExpression,
    ExpressionKind as HirExpressionKind, Item, ItemKind, Literal, Module, SourceGoal, SourceKind,
    Span, Statement as HirStatement, StatementKind as HirStatementKind,
};

/// Deliberately tiny pools, for the reason the original gives: collisions and reversions are the
/// interesting inputs -- re-writing a file's previous text, or a body edit that leaves the
/// interface alone -- and a wide pool makes them vanishingly rare.
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

/// The suite supplies its own frontend rather than borrowing a real one.
///
/// `MemoEngine::new` takes the frontend as a parameter (ADR 0006), and the property under test is
/// a claim about memoization, not about parsing. A trivial deterministic `(path, source) -> Module`
/// exercises it equally and means a frontend change can never fail this for the wrong reason.
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
            .find_map(|keyword| rest.strip_prefix(keyword))
            .unwrap_or(rest);
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|character| {
                character.is_alphanumeric() || *character == '_' || *character == '$'
            })
            .collect();
        if !name.is_empty() {
            exports.push(name);
        }
    }

    Module {
        source: std::sync::Arc::from(&text[..]),
        strict: false,
        source_kind: SourceKind::JavaScript,
        source_goal: SourceGoal::Module,
        items: text
            .lines()
            .enumerate()
            .map(|(line, _)| Item {
                kind: ItemKind::Other,
                span: Span::new(line as u32, line as u32 + 1),
            })
            .collect(),
        // A body that varies with the source. An empty one would make every text in the pool
        // compile to identical bytecode and turn the `irs` comparison below into a comparison of
        // a constant -- see `the_text_pool_produces_distinguishable_ir`.
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

/// One step of a generated history.
///
/// Indices are reduced modulo the live state when the step runs, so a mutated or shrunk sequence
/// stays executable instead of addressing a file that no longer exists. That is what lets a
/// coverage-guided engine mutate a history byte at a time without producing garbage.
#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum Op {
    Intern(u8),
    SetSource(u8, u8),
    Parse(u8),
    Interface(u8),
    Ir(u8),
    Project,
}

/// A generated history. Capped when it runs, not when it is generated.
#[derive(Debug, Clone, Arbitrary)]
pub struct History(pub crate::Many<Op, 24>);

/// A handful of files and tens of operations: the bugs this is aimed at need a couple of edits
/// and a stale memo, not a large project.
const MAX_STEPS: usize = 24;

/// Everything a caller can observe, so that "the engines agree" is one comparison rather than a
/// checklist a future query could fall off.
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    sources: Vec<String>,
    parses: Vec<Parsed>,
    /// Memoized independently of `interface`, with its own counter, so it needs its own
    /// comparison: an engine that memoized `ir` wrongly while keeping `parse` and `interface`
    /// correct would satisfy every other field here.
    irs: Vec<Compiled>,
    interfaces: Vec<Interface>,
    project: Project,
}

/// `top_down` forces the rollup before the leaves. The two orders exercise different halves of
/// the algorithm -- a validation walk that has to recurse, versus one that finds its dependencies
/// already verified -- and alternating them costs nothing.
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
/// a first write is a real edit even when the text is `""`.
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

fn apply(db: &mut MemoEngine, inputs: &mut Vec<(String, Option<String>)>, op: Op) {
    match op {
        Op::Intern(slot) => {
            let path = PATHS[slot as usize % PATHS.len()];
            if !inputs.iter().any(|(known, _)| known == path) {
                inputs.push((path.to_owned(), None));
            }
            db.intern(path);
        }
        Op::SetSource(slot, text) if !inputs.is_empty() => {
            let slot = slot as usize % inputs.len();
            let text = TEXTS[text as usize % TEXTS.len()];
            inputs[slot].1 = Some(text.to_owned());
            db.set_source(FileId(slot as u32), text.to_owned());
        }
        Op::Parse(slot) if !inputs.is_empty() => {
            db.parse(FileId((slot as usize % inputs.len()) as u32));
        }
        Op::Interface(slot) if !inputs.is_empty() => {
            db.interface(FileId((slot as usize % inputs.len()) as u32));
        }
        Op::Ir(slot) if !inputs.is_empty() => {
            // `Compiled` is a `Result`; a compile failure is a legitimate answer to memoize,
            // not an error to handle here.
            let _ = db.ir(FileId((slot as usize % inputs.len()) as u32));
        }
        Op::Project => {
            db.project();
        }
        // A file-addressed step before anything is interned has nothing to address.
        Op::SetSource(..) | Op::Parse(_) | Op::Interface(_) | Op::Ir(_) => {}
    }
}

/// Why a history disagreed. Returned rather than panicked so the caller decides how to report,
/// and so the message carries the history that produced it.
#[derive(Debug)]
pub struct Disagreement(pub String);

/// Memoized equals from-scratch, after every single step.
pub fn memoized_agrees_at_every_step(history: &History) -> Result<(), Disagreement> {
    let mut db = MemoEngine::new(oracle_frontend);
    let mut inputs: Vec<(String, Option<String>)> = Vec::new();
    let mut warm = false;

    for (step, op) in history.0.iter().take(MAX_STEPS).enumerate() {
        let interfaces_before: Vec<Interface> = (0..inputs.len())
            .map(|index| db.interface(FileId(index as u32)))
            .collect();
        let files_before = inputs.len();
        let active_before: Vec<bool> = inputs.iter().map(|(_, text)| text.is_some()).collect();
        db.reset_stats();

        apply(&mut db, &mut inputs, *op);

        let observed = observe(&mut db, inputs.len(), step % 2 == 0);
        let project_computed = db.stats().project_computed;

        let mut oracle = from_scratch(&inputs);
        let expected = observe(&mut oracle, inputs.len(), step % 2 == 1);
        if observed != expected {
            return Err(Disagreement(format!(
                "step {step} ({op:?}) disagrees with a fresh engine\n  memoized: {observed:#?}\n  \
                 fresh:    {expected:#?}\n  history:  {:?}",
                history.0
            )));
        }

        // The rollup reads the file set, every interface, and runtime linkage, so if none moved,
        // no amount of editing underneath them may reach it. Activating an interned-but-unset
        // path changes the file set even if its interface is empty.
        let active_after: Vec<bool> = inputs.iter().map(|(_, text)| text.is_some()).collect();
        if warm
            && inputs.len() == files_before
            && active_after == active_before
            && observed.interfaces == interfaces_before
            && project_computed != 0
        {
            return Err(Disagreement(format!(
                "step {step} ({op:?}) recomputed the rollup with no interface changed; history: {:?}",
                history.0
            )));
        }
        warm = true;

        identical_rewrite_is_free(&mut db, &inputs)?;
    }
    Ok(())
}

/// The same histories with the per-step comparison removed.
///
/// Comparing after every step leaves the engine fully verified, which is the state in which a
/// stale memo is hardest to catch: a history whose edits pile up before anything reads them
/// never occurs. Here only the generated queries run, so several edits land between reads and a
/// memo has room to go quietly wrong.
pub fn memoized_agrees_after_a_batch(history: &History) -> Result<(), Disagreement> {
    let mut db = MemoEngine::new(oracle_frontend);
    let mut inputs: Vec<(String, Option<String>)> = Vec::new();
    for op in history.0.iter().take(MAX_STEPS) {
        apply(&mut db, &mut inputs, *op);
    }

    let observed = observe(&mut db, inputs.len(), true);
    let mut oracle = from_scratch(&inputs);
    let expected = observe(&mut oracle, inputs.len(), false);
    if observed == expected {
        return Ok(());
    }
    Err(Disagreement(format!(
        "batched history diverged: {:?}\n  memoized: {observed:#?}\n  fresh: {expected:#?}",
        history.0
    )))
}

/// Re-sending state the engine already holds must cost nothing at all -- not one parse per file.
/// That is what makes a stateless client safe to point at a warm daemon, and it is a property of
/// the inputs rather than of the cache, so it holds at every point in every history.
fn identical_rewrite_is_free(
    db: &mut MemoEngine,
    inputs: &[(String, Option<String>)],
) -> Result<(), Disagreement> {
    let revision = db.revision();
    db.reset_stats();
    // Only files that have actually been written: re-sending `""` to a file that was interned
    // but never written is a first write, not a rewrite, and is correctly an edit.
    for (index, (_, text)) in inputs.iter().enumerate() {
        if let Some(text) = text {
            db.set_source(FileId(index as u32), text.clone());
        }
    }

    let stats = db.stats();
    let expected = Stats {
        revision: revision.0,
        ..Stats::default()
    };
    if stats != expected || db.revision() != revision {
        return Err(Disagreement(format!(
            "a byte-identical write is not an edit and must move no counter: {stats:?}"
        )));
    }
    Ok(())
}

/// Guards the guard: a field in `Observed` is only worth comparing if it can actually differ.
///
/// If `ir` were constant across the text pool then comparing it would pass unconditionally and
/// the coverage would be an illusion. Callable so a test can fail loudly instead of quietly
/// weakening the oracle.
pub fn text_pool_produces_distinguishable_ir() -> bool {
    let mut db = MemoEngine::new(oracle_frontend);
    let file = db.intern("a.js");

    let mut distinct: Vec<Compiled> = Vec::new();
    for text in TEXTS {
        db.set_source(file, text.to_owned());
        let compiled = db.ir(file);
        if !distinct.contains(&compiled) {
            distinct.push(compiled);
        }
    }
    distinct.len() > 1
}
