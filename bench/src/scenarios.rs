//! The measured operations from design.md §13, for the subset that exists in v0.
//!
//! Everything here is written against the [`QueryEngine`] trait rather than a concrete backend,
//! so the same harness can be pointed at a second implementation (docs/adr/0002) without being
//! rewritten. `new_engine` is the only line that names the backend.

use std::time::{Duration, Instant};

use kiln_db::{FileId, MemoEngine, QueryEngine, Stats as DbStats};

use crate::report::{Measurement, Scenario};
use crate::timing::Samples;
use crate::workload::{EditKind, Workload};

fn new_engine() -> impl QueryEngine {
    MemoEngine::new()
}

/// What one warm iteration does. Named here because the human report prints it: a latency
/// number is meaningless without saying which work it covers.
pub const WARM_PIPELINE: &str = "set_source(edited) -> parse(edited) -> project()";

/// Iteration budgets. Warm edits are cheap and noisy, so they get more samples; the cold
/// scenarios rebuild the whole project every iteration and get fewer.
const STARTUP_WARMUP: usize = 3;
const STARTUP_ITERS: usize = 12;
const COLD_WARMUP: usize = 2;
const COLD_ITERS: usize = 10;
const WARM_WARMUP: usize = 5;
const WARM_ITERS: usize = 25;

/// Overrides from the command line, applied to every scenario when present.
#[derive(Debug, Clone, Copy, Default)]
pub struct Budget {
    pub warmup: Option<usize>,
    pub iterations: Option<usize>,
}

impl Budget {
    fn apply(self, warmup: usize, iterations: usize) -> (usize, usize) {
        (
            self.warmup.unwrap_or(warmup),
            self.iterations.unwrap_or(iterations).max(1),
        )
    }
}

pub fn run_all(w: &Workload, budget: Budget) -> Result<Vec<Measurement>, String> {
    Ok(vec![
        startup(w, budget),
        cold(w, budget),
        warm(w, EditKind::Implementation, budget)?,
        warm(w, EditKind::Interface, budget)?,
    ])
}

/// Cost of standing a session up: intern every path and ingest every source, deriving nothing.
///
/// This is the closest v0 analogue of "daemon startup"; see `report`'s footnote for why it is
/// not yet the real thing.
fn startup(w: &Workload, budget: Budget) -> Measurement {
    let (warmup, iters) = budget.apply(STARTUP_WARMUP, STARTUP_ITERS);
    let paths = paths(w);
    let mut ids = Vec::with_capacity(paths.len());
    let mut acc = Accumulator::new(warmup, iters);

    for _ in 0..warmup + iters {
        let payload = w.base_texts.clone();
        let mut e = new_engine();
        let start = Instant::now();
        load(&mut e, &paths, payload, &mut ids);
        acc.push(start.elapsed(), e.stats());
    }

    acc.finish(w, "startup", "daemon startup", None)
}

/// Everything from an empty database to a whole-project answer.
fn cold(w: &Workload, budget: Budget) -> Measurement {
    let (warmup, iters) = budget.apply(COLD_WARMUP, COLD_ITERS);
    let paths = paths(w);
    let mut ids = Vec::with_capacity(paths.len());
    let mut acc = Accumulator::new(warmup, iters);

    for _ in 0..warmup + iters {
        let payload = w.base_texts.clone();
        let mut e = new_engine();
        let start = Instant::now();
        load(&mut e, &paths, payload, &mut ids);
        e.project();
        acc.push(start.elapsed(), e.stats());
    }

    acc.finish(w, "cold", "cold full analysis", None)
}

/// The loop that actually matters: warm daemon, edit one module, re-run the pipeline.
///
/// Iterations alternate between two variants of the target module so that every one of them is
/// a real edit in both directions. Editing once and then re-running would measure a no-op.
fn warm(w: &Workload, kind: EditKind, budget: Budget) -> Result<Measurement, String> {
    let (warmup, iters) = budget.apply(WARM_WARMUP, WARM_ITERS);
    let paths = paths(w);
    let target = w.edit_target;

    let mut ids = Vec::with_capacity(paths.len());
    let mut e = new_engine();
    load(&mut e, &paths, w.base_texts.clone(), &mut ids);
    e.project();
    let file = ids[target];

    check_edit_classification(&mut e, w, file, kind)?;

    let mut acc = Accumulator::new(warmup, iters);
    for i in 0..warmup + iters {
        // 1 and 2, never 0: returning to the base text would make alternate iterations
        // asymmetric, because only one of the two directions would be a fresh value.
        let text = w.variant(target, kind, 1 + (i % 2) as u32);
        e.reset_stats();
        let start = Instant::now();
        e.set_source(file, text);
        e.parse(file);
        e.project();
        acc.push(start.elapsed(), e.stats());
    }

    Ok(acc.finish(w, kind.slug(), kind.label(), Some(w.spec.warm_goal_ms)))
}

/// Refuse to report a number for a scenario that is not measuring what it claims to.
///
/// If an "implementation edit" moves the interface hash, the two warm figures are not
/// comparable and the headline of the whole report - the ratio between them - is a lie.
fn check_edit_classification(
    e: &mut impl QueryEngine,
    w: &Workload,
    file: FileId,
    kind: EditKind,
) -> Result<(), String> {
    let before = e.interface(file).interface_hash;
    e.set_source(file, w.variant(w.edit_target, kind, 1));
    let after = e.interface(file).interface_hash;
    e.project();

    let changed = before != after;
    match kind {
        EditKind::Implementation if changed => Err(format!(
            "workload {}: the implementation-only edit to {} changed the interface hash \
             ({before:#x} -> {after:#x}); the fixture and the frontend disagree about what is \
             externally visible, so the warm numbers would not be comparable",
            w.spec.name,
            w.modules[w.edit_target].path()
        )),
        EditKind::Interface if !changed => Err(format!(
            "workload {}: the interface edit to {} left the interface hash at {before:#x}; \
             either the frontend is not seeing the added export or early cutoff is too eager",
            w.spec.name,
            w.modules[w.edit_target].path()
        )),
        _ => Ok(()),
    }
}

fn paths(w: &Workload) -> Vec<String> {
    w.modules.iter().map(|m| m.path()).collect()
}

/// Intern and ingest every module, recording the ids in `ids`.
///
/// Interning is inside the caller's timer because it is real engine work; allocating the
/// payload is not, so callers clone the texts and reserve `ids` beforehand. The ids come back
/// from `intern` rather than being assumed to be `0..n` - that numbering is the engine's
/// business, not the contract's.
fn load(e: &mut impl QueryEngine, paths: &[String], texts: Vec<String>, ids: &mut Vec<FileId>) {
    ids.clear();
    for (path, text) in paths.iter().zip(texts) {
        let file = e.intern(path);
        e.set_source(file, text);
        ids.push(file);
    }
}

/// Collects timings and recompute counts, discarding the warmup prefix.
struct Accumulator {
    warmup: usize,
    seen: usize,
    durations: Vec<Duration>,
    stats: Vec<DbStats>,
}

impl Accumulator {
    fn new(warmup: usize, iterations: usize) -> Accumulator {
        Accumulator {
            warmup,
            seen: 0,
            durations: Vec::with_capacity(iterations),
            stats: Vec::with_capacity(iterations),
        }
    }

    fn push(&mut self, elapsed: Duration, stats: DbStats) {
        self.seen += 1;
        if self.seen > self.warmup {
            self.durations.push(elapsed);
            self.stats.push(stats);
        }
    }

    fn finish(
        self,
        w: &Workload,
        slug: &str,
        operation: &str,
        goal_ms: Option<f64>,
    ) -> Measurement {
        // The revision counter climbs by construction; every other counter should be identical
        // on every measured iteration, and if it is not, the loop never reached a steady state.
        let steady = self
            .stats
            .windows(2)
            .all(|p| recompute_counters(&p[0]) == recompute_counters(&p[1]));
        Measurement::new(
            Scenario {
                workload: w.spec.name,
                slug,
                operation,
                goal_ms,
            },
            self.warmup,
            Samples::from(&self.durations),
            self.stats[0],
            steady,
        )
    }
}

fn recompute_counters(s: &DbStats) -> [u64; 5] {
    [
        s.parse_computed,
        s.interface_computed,
        s.project_computed,
        s.validated,
        s.early_cutoff,
    ]
}
