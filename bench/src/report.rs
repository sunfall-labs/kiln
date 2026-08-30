//! Report construction, rendering and baseline comparison.

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use kiln_db::Stats as DbStats;
use kiln_proto::Stats;
use serde::{Deserialize, Serialize};

use crate::timing::Samples;
use crate::workload::Workload;

/// Bumped when the JSON shape changes incompatibly, so `--compare` can refuse a stale baseline
/// instead of silently comparing fields that no longer mean the same thing.
pub const SCHEMA_VERSION: u32 = 1;

pub const GOALS_NOTE: &str = "PROVISIONAL. design.md §13 budgets a warm small JS edit at \
200 ms and a warm medium edit at 500 ms, mapped here onto the small and medium workloads. §13 \
also says they need calibrating on a fixed reference machine, and this run was not taken on \
one - so read a verdict as a smoke signal and the --compare delta as the real gate.";

/// A percentage regression under this many milliseconds is not distinguishable from scheduling
/// noise, and gating CI on it would only teach people to ignore the gate.
const NOISE_FLOOR_MS: f64 = 0.25;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema: u32,
    pub generated_unix_secs: u64,
    pub machine: Machine,
    pub warm_pipeline: String,
    pub workloads: Vec<WorkloadInfo>,
    pub measurements: Vec<Measurement>,
}

impl Report {
    pub fn new(warm_pipeline: &str) -> Report {
        Report {
            schema: SCHEMA_VERSION,
            generated_unix_secs: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default(),
            machine: Machine::detect(),
            warm_pipeline: warm_pipeline.to_string(),
            workloads: Vec::new(),
            measurements: Vec::new(),
        }
    }

    fn get(&self, id: &str) -> Option<&Measurement> {
        self.measurements.iter().find(|m| m.id == id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Machine {
    pub os: String,
    pub arch: String,
    pub cpus: usize,
    /// `debug` or `release`. Recorded because it moves every number here by an order of
    /// magnitude, and a baseline taken under a different profile is worthless.
    pub build_profile: String,
}

impl Machine {
    fn detect() -> Machine {
        Machine {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpus: std::thread::available_parallelism().map_or(0, |n| n.get()),
            build_profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .to_string(),
        }
    }

    fn describe(&self) -> String {
        format!(
            "{}/{}, {} cpus, {} build",
            self.os, self.arch, self.cpus, self.build_profile
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadInfo {
    pub name: String,
    pub modules: usize,
    pub layers: usize,
    pub import_edges: usize,
    pub source_lines: usize,
    pub edit_target: String,
    pub edit_target_dependents: usize,
}

impl WorkloadInfo {
    pub fn of(w: &Workload) -> WorkloadInfo {
        WorkloadInfo {
            name: w.spec.name.to_string(),
            modules: w.modules.len(),
            layers: w.spec.layers,
            import_edges: w.import_edges(),
            source_lines: w.total_lines(),
            edit_target: w.modules[w.edit_target].path(),
            edit_target_dependents: w.edit_target_dependents(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measurement {
    /// `<workload>/<operation-slug>`; the key `--compare` matches on.
    pub id: String,
    pub workload: String,
    pub operation: String,
    pub goal_ms: Option<f64>,
    pub warmup_iterations: usize,
    pub iterations: usize,
    pub median_ms: f64,
    pub min_ms: f64,
    pub p90_ms: f64,
    pub max_ms: f64,
    /// Recompute counts for one measured iteration. The whole point of the design is that this
    /// stays flat, so it is reported next to the wall clock rather than below it.
    pub stats: Stats,
    pub cache_hit_rate: f64,
    /// False if the recompute counts differed between measured iterations, which means the loop
    /// never reached a steady state and the timings describe a mixture of different work.
    pub steady_state: bool,
    /// Native-only artifact details. Omitted for query/interpreter measurements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeArtifactInfo>,
}

/// Artifact facts attached to a native benchmark measurement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeArtifactInfo {
    pub cache_hit: bool,
    /// Generated C bytes for application/module units (runtime/launcher C is shared support code).
    pub generated_c_bytes: u64,
    /// Aggregate application/module, runtime, and launcher object bytes.
    pub object_bytes: u64,
    pub executable_bytes: u64,
}

/// How a scenario names and budgets itself, kept apart from what it measured.
pub struct Scenario<'a> {
    pub workload: &'a str,
    /// Combined with the workload name into the stable `--compare` key.
    pub slug: &'a str,
    /// Human label for the tables.
    pub operation: &'a str,
    pub goal_ms: Option<f64>,
}

impl Measurement {
    pub fn new(
        scenario: Scenario<'_>,
        warmup_iterations: usize,
        samples: Samples,
        stats: DbStats,
        steady_state: bool,
    ) -> Measurement {
        Measurement {
            id: format!("{}/{}", scenario.workload, scenario.slug),
            workload: scenario.workload.to_string(),
            operation: scenario.operation.to_string(),
            goal_ms: scenario.goal_ms,
            warmup_iterations,
            iterations: samples.len(),
            median_ms: samples.median(),
            min_ms: samples.min(),
            p90_ms: samples.percentile(0.9),
            max_ms: samples.max(),
            cache_hit_rate: cache_hit_rate(&stats),
            stats: convert(stats),
            steady_state,
            native: None,
        }
    }

    fn recomputes(&self) -> u64 {
        self.stats.parse_computed
            + self.stats.ir_computed
            + self.stats.interface_computed
            + self.stats.project_computed
    }

    fn verdict(&self) -> &'static str {
        match self.goal_ms {
            Some(goal) if self.median_ms <= goal => "pass",
            Some(_) => "OVER",
            None => "-",
        }
    }
}

/// Queries answered from cache as a fraction of all queries touched.
///
/// Spelled out because the phrase is ambiguous: the denominator is every query the pipeline
/// visited, and a "hit" is a query the engine validated without recomputing.
fn cache_hit_rate(s: &DbStats) -> f64 {
    let recomputed = s.parse_computed + s.ir_computed + s.interface_computed + s.project_computed;
    let total = s.validated + recomputed;
    if total == 0 {
        0.0
    } else {
        s.validated as f64 / total as f64
    }
}

/// `kiln_db::Stats` is deliberately dependency-free, so the wire type is the one that gets
/// serialised; keeping a single serialised shape means the daemon and the harness agree.
fn convert(s: DbStats) -> Stats {
    Stats {
        revision: s.revision,
        parse_computed: s.parse_computed,
        ir_computed: s.ir_computed,
        interface_computed: s.interface_computed,
        project_computed: s.project_computed,
        validated: s.validated,
        early_cutoff: s.early_cutoff,
        ..Stats::default()
    }
}

pub fn render_human(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "kiln-bench - iteration-speed harness (design.md §13)\n"
    );
    let _ = writeln!(out, "  machine   {}", report.machine.describe());
    let _ = writeln!(out, "  pipeline  warm iteration = {}", report.warm_pipeline);
    let _ = writeln!(out, "  goals     {}\n", wrap(GOALS_NOTE, 68, 12));
    if report.machine.build_profile == "debug" {
        let _ = writeln!(
            out,
            "  WARNING   this is a debug build; the timings below say nothing about the goals.\n"
        );
    }

    render_standalone(&mut out, report, "daemon", "daemon");
    for info in &report.workloads {
        render_workload(&mut out, report, info);
    }
    render_standalone(&mut out, report, "conformance", "conformance");
    render_standalone(&mut out, report, "native", "native");
    out.push_str(NOTES);
    out
}

/// Everything a reader needs in order not to over-read the table above.
const NOTES: &str = "\
notes
  \"daemon process startup + socket handshake\" launches the sibling `kiln` CLI with a unique
  socket, waits through daemon process creation and the protocol handshake, and stops that daemon
  outside the timed region. A failed start, handshake, or cleanup fails the harness.

  \"engine ingest floor\" constructs the query engine and installs every workload source while
  deriving nothing. It isolates source-database population and is not labelled daemon startup.

  \"cache hit rate\" is validated / (validated + recomputed) over every memo the pipeline
  touched, so a query validated on the way down a dependency walk counts as a hit.

  v0's query layer is represented by memoized scalar and linked-module queries.
  The generated import graph is exercised by linked rows when those rows are present.

  \"warm-impl-edit\" and \"warm-interface-edit\" are scalar database-query measurements: they
  edit one source, query parse/interface/project, and do not build or execute a module graph.
  Their import edges describe the generated workload, but do not make those rows graph
  measurements.

  \"warm-execute\" is scalar interpreter execution: edit, parse/HIR lowering, scalar bytecode
  rebuild, and VM launch for one module. It is distinct from linked interpreter rows, which build
  an entry-rooted graph and execute dependencies before the entry.

  Linked interpreter rows rebuild a fresh entry-rooted graph on every iteration. The graph
  traversal, resolution, link validation, and graph allocation are not memoized; the provider can
  still reuse memoized per-module module_ir. Consequently DB recompute/hit counters describe the
  query layer only and do not imply that graph traversal was skipped. These rows use a scalar
  mirror of the reported workload topology plus one synthetic entry that imports every reported
  module; the established function-shaped analysis workload and its source-line count remain
  unchanged for historical comparisons. The displayed \"IR\" counter is the database's combined
  scalar-ir/module-ir counter; in a linked-only row, a value of one means one module_ir rebuild.
  Linked rows have no provisional latency goal until a fixed-machine baseline calibrates one.

  \"targeted Test262\" runs an in-memory positive and parse-negative batch through metadata,
  explicit variants, the parser, bytecode compiler and interpreter. Full-suite throughput
  remains a separate measurement once the upstream harness is supported.

  Native linked-graph rows include a fresh entry-rooted graph build before real per-module
  C/object/link work and artifact-cache lookup; they are not lookup-only microbenchmarks or graph
  cache hits. The warm unchanged row requires every module, runtime, launcher, and executable
  artifact to hit. The body-edit row requires exactly one module C/object miss, a warm launcher,
  and a relink; it starts from precompiled module IR to isolate the native seam. Generated-C size
  covers application modules, while object size aggregates application, runtime, and launcher
  units. Native equivalence compares linked interpreter and native stdout.

";

fn render_standalone(out: &mut String, report: &Report, workload: &str, label: &str) {
    let measurements = report
        .measurements
        .iter()
        .filter(|measurement| measurement.workload == workload)
        .collect::<Vec<_>>();
    if measurements.is_empty() {
        return;
    }
    let _ = writeln!(out, "{label}");
    render_linked_note(out, &measurements);
    let _ = writeln!(
        out,
        "  {:<42} {:<23} {:>10} {:>10} {:>10} {:>10} {:>6}",
        "operation", "kind", "median", "min", "p90", "max", "iters"
    );
    let _ = writeln!(out, "  {}", "-".repeat(120));
    for measurement in measurements {
        let _ = writeln!(
            out,
            "  {:<42} {:<23} {:>10} {:>10} {:>10} {:>10} {:>6}",
            measurement.operation,
            measurement_kind(measurement),
            ms(measurement.median_ms),
            ms(measurement.min_ms),
            ms(measurement.p90_ms),
            ms(measurement.max_ms),
            measurement.iterations,
        );
        if let Some(native) = &measurement.native {
            let cache = if native.cache_hit { "hit" } else { "miss" };
            let _ = writeln!(
                out,
                "    artifacts: cache {cache}, application C {} B, all objects {} B, executable {} B",
                native.generated_c_bytes, native.object_bytes, native.executable_bytes
            );
        }
    }
    out.push('\n');
}

fn render_workload(out: &mut String, report: &Report, info: &WorkloadInfo) {
    let _ = writeln!(
        out,
        "workload \"{}\": {} modules over {} layers, {} imports, {} lines",
        info.name, info.modules, info.layers, info.import_edges, info.source_lines
    );
    let _ = writeln!(
        out,
        "  edited module: {} ({} direct dependents)\n",
        info.edit_target, info.edit_target_dependents
    );

    let mine: Vec<&Measurement> = report
        .measurements
        .iter()
        .filter(|m| m.workload == info.name)
        .collect();

    render_linked_note(out, &mine);
    let _ = writeln!(
        out,
        "  {:<28} {:<23} {:>10} {:>10} {:>10} {:>10} {:>10} {:>6} {:>8}",
        "operation", "kind", "median", "min", "p90", "max", "goal", "iters", "vs goal"
    );
    let _ = writeln!(out, "  {}", "-".repeat(126));
    for m in &mine {
        let goal = m.goal_ms.map_or_else(|| "-".to_string(), ms);
        let _ = writeln!(
            out,
            "  {:<28} {:<23} {:>10} {:>10} {:>10} {:>10} {:>10} {:>6} {:>8}",
            m.operation,
            measurement_kind(m),
            ms(m.median_ms),
            ms(m.min_ms),
            ms(m.p90_ms),
            ms(m.max_ms),
            goal,
            m.iterations,
            m.verdict(),
        );
    }
    let warmups: Vec<String> = mine
        .iter()
        .map(|m| format!("{}={}", m.operation, m.warmup_iterations))
        .collect();
    let _ = writeln!(
        out,
        "  warmup iterations discarded: {}\n",
        warmups.join(", ")
    );

    let _ = writeln!(out, "  recompute counts for one measured iteration");
    let _ = writeln!(
        out,
        "  {:<24} {:>7} {:>5} {:>10} {:>8} {:>10} {:>13} {:>9}",
        "operation", "parse", "IR", "interface", "project", "validated", "early cutoff", "hit rate"
    );
    let _ = writeln!(out, "  {}", "-".repeat(100));
    for m in &mine {
        let _ = writeln!(
            out,
            "  {:<24} {:>7} {:>5} {:>10} {:>8} {:>10} {:>13} {:>8.1}%",
            m.operation,
            m.stats.parse_computed,
            m.stats.ir_computed,
            m.stats.interface_computed,
            m.stats.project_computed,
            m.stats.validated,
            m.stats.early_cutoff,
            m.cache_hit_rate * 100.0,
        );
        if !m.steady_state {
            let _ = writeln!(
                out,
                "  {:<24} !! recompute counts varied between iterations; not a steady state",
                ""
            );
        }
    }
    out.push('\n');

    render_thesis(out, report, &info.name);
}

/// Linked rows exercise the real entry-rooted module graph. The graph itself is deliberately not
/// a database query yet: every iteration builds a fresh traversal, while each provider may reuse
/// memoized per-module `module_ir`. Keep this distinction visible beside the timings so DB cache
/// counters are not mistaken for graph-traversal counters.
fn render_linked_note(out: &mut String, measurements: &[&Measurement]) {
    if measurements
        .iter()
        .any(|measurement| is_linked(measurement))
    {
        let _ = writeln!(
            out,
            "  linked graph rows rebuild a fresh entry-rooted graph each iteration over memoized per-module module_ir where available; traversal and link validation are not memoized"
        );
    }
}

fn is_linked(measurement: &Measurement) -> bool {
    let id = measurement.id.to_ascii_lowercase();
    let operation = measurement.operation.to_ascii_lowercase();
    id.contains("linked")
        || id.contains("graph")
        || operation.contains("linked")
        || operation.contains("graph")
}

/// Human-only classification. This is intentionally derived from IDs/labels rather than added to
/// `Measurement`, so the JSON shape and baseline compatibility stay unchanged.
fn measurement_kind(measurement: &Measurement) -> &'static str {
    if measurement.native.is_some() || measurement.workload == "native" {
        if is_linked(measurement) {
            "native linked graph"
        } else {
            "native build"
        }
    } else if is_linked(measurement) {
        "linked interpreter"
    } else if measurement.workload == "daemon" {
        "process lifecycle"
    } else if measurement.workload == "conformance" {
        "conformance"
    } else {
        let id = measurement.id.to_ascii_lowercase();
        let operation = measurement.operation.to_ascii_lowercase();
        if id.contains("execute") || operation.contains("interpreted") {
            "scalar interpreter"
        } else {
            "incremental queries"
        }
    }
}

/// The side-by-side the whole design turns on.
fn render_thesis(out: &mut String, report: &Report, workload: &str) {
    let (Some(imp), Some(iface)) = (
        report.get(&format!("{workload}/warm-impl-edit")),
        report.get(&format!("{workload}/warm-interface-edit")),
    ) else {
        return;
    };

    let _ = writeln!(out, "  implementation edit vs interface edit");
    let _ = writeln!(
        out,
        "    latency          {} vs {}  ({:.2}x)",
        ms(imp.median_ms),
        ms(iface.median_ms),
        ratio(iface.median_ms, imp.median_ms),
    );
    let _ = writeln!(
        out,
        "    queries recomputed  {} vs {}",
        imp.recomputes(),
        iface.recomputes()
    );
    let held = imp.stats.project_computed == 0;
    let _ = writeln!(
        out,
        "    early cutoff held on the implementation edit: {}{}",
        if held { "yes" } else { "NO" },
        if held {
            ""
        } else {
            "  <- the project rollup was recomputed for an edit no dependent can observe"
        }
    );
    out.push('\n');
}

fn ratio(a: f64, b: f64) -> f64 {
    if b == 0.0 { f64::INFINITY } else { a / b }
}

fn ms(v: f64) -> String {
    if v < 1.0 {
        format!("{v:.3} ms")
    } else if v < 1000.0 {
        format!("{v:.2} ms")
    } else {
        format!("{v:.1} ms")
    }
}

/// Hanging-indent wrap for the notes in the header block.
fn wrap(text: &str, width: usize, indent: usize) -> String {
    let mut out = String::new();
    let mut col = 0;
    for word in text.split_whitespace() {
        if col > 0 && col + 1 + word.len() > width {
            out.push('\n');
            out.push_str(&" ".repeat(indent));
            col = 0;
        } else if col > 0 {
            out.push(' ');
            col += 1;
        }
        out.push_str(word);
        col += word.len();
    }
    out
}

#[derive(Debug)]
pub struct Regression {
    pub id: String,
    pub detail: String,
}

/// Compare a run against a baseline, returning the rendered comparison and any regressions.
///
/// Three independent gates. Wall clock has a threshold and a noise floor because it is a noisy
/// measurement; recompute counts have neither, because they are exact and an edit that starts
/// recomputing more work has regressed no matter what the clock says. Both sides must also be
/// steady-state measurements: comparing timings that describe different work is invalid even if
/// their first-iteration counters happen to match.
pub fn compare(run: &Report, baseline: &Report, threshold: f64) -> (String, Vec<Regression>) {
    let mut out = String::new();
    let mut regressions = Vec::new();

    let _ = writeln!(
        out,
        "comparison against baseline (threshold {:.0}%, noise floor {NOISE_FLOOR_MS} ms)",
        threshold * 100.0
    );
    if baseline.machine.build_profile != run.machine.build_profile
        || baseline.machine.arch != run.machine.arch
    {
        let _ = writeln!(
            out,
            "  NOTE: baseline was taken on {} - the wall-clock comparison below is not meaningful",
            baseline.machine.describe()
        );
    }
    let _ = writeln!(out, "  {}", "-".repeat(92));

    for m in &run.measurements {
        let Some(base) = baseline.get(&m.id) else {
            let _ = writeln!(out, "  {:<34} new, not in baseline", m.id);
            continue;
        };

        let delta = m.median_ms - base.median_ms;
        let pct = if base.median_ms == 0.0 {
            0.0
        } else {
            delta / base.median_ms
        };
        let slower = pct > threshold && delta > NOISE_FLOOR_MS;
        let recomputed = m.recomputes();
        let base_recomputed = base.recomputes();
        let unstable = !m.steady_state || !base.steady_state;

        let _ = writeln!(
            out,
            "  {:<34} {:>10} vs {:>10}  {:>+7.1}%   recompute {} vs {}   {}",
            m.id,
            ms(m.median_ms),
            ms(base.median_ms),
            pct * 100.0,
            recomputed,
            base_recomputed,
            if slower || recomputed > base_recomputed || unstable {
                "REGRESSION"
            } else {
                "ok"
            },
        );

        if slower {
            regressions.push(Regression {
                id: m.id.clone(),
                detail: format!(
                    "median {} vs {} baseline ({:+.1}%, threshold {:.0}%)",
                    ms(m.median_ms),
                    ms(base.median_ms),
                    pct * 100.0,
                    threshold * 100.0
                ),
            });
        }
        if recomputed > base_recomputed {
            regressions.push(Regression {
                id: m.id.clone(),
                detail: format!(
                    "recomputed {recomputed} queries vs {base_recomputed} in the baseline"
                ),
            });
        }
        if !m.steady_state {
            regressions.push(Regression {
                id: m.id.clone(),
                detail:
                    "measurement is not steady-state: recompute counts varied between iterations"
                        .to_string(),
            });
        }
        if !base.steady_state {
            regressions.push(Regression {
                id: m.id.clone(),
                detail: "baseline is not steady-state: recompute counts varied between iterations"
                    .to_string(),
            });
        }
    }

    for base in &baseline.measurements {
        if run.get(&base.id).is_none() {
            let _ = writeln!(
                out,
                "  {:<34} in baseline but not measured this run   REGRESSION",
                base.id
            );
            regressions.push(Regression {
                id: base.id.clone(),
                detail: "measurement is present in the baseline but missing from the current run"
                    .to_string(),
            });
        }
    }
    out.push('\n');

    (out, regressions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measurement(id: &str, median_ms: f64, recomputes: u64) -> Measurement {
        Measurement {
            id: id.to_string(),
            workload: "small".to_string(),
            operation: "warm".to_string(),
            goal_ms: Some(200.0),
            warmup_iterations: 5,
            iterations: 25,
            median_ms,
            min_ms: median_ms,
            p90_ms: median_ms,
            max_ms: median_ms,
            stats: Stats {
                project_computed: recomputes,
                validated: 10,
                ..Stats::default()
            },
            cache_hit_rate: 0.5,
            steady_state: true,
            native: None,
        }
    }

    fn report(ms: f64, recomputes: u64) -> Report {
        let mut r = Report::new("test");
        r.measurements
            .push(measurement("small/warm-impl-edit", ms, recomputes));
        r
    }

    #[test]
    fn a_large_relative_slowdown_below_the_noise_floor_is_not_a_regression() {
        // 0.01 ms -> 0.05 ms is +400%, and is also four hundredths of a millisecond.
        let (_, regressions) = compare(&report(0.05, 1), &report(0.01, 1), 0.15);
        assert!(regressions.is_empty(), "{regressions:?}");
    }

    #[test]
    fn a_slowdown_past_the_threshold_and_the_floor_is_a_regression() {
        let (_, regressions) = compare(&report(12.0, 1), &report(10.0, 1), 0.15);
        assert_eq!(regressions.len(), 1);
        assert!(regressions[0].detail.contains("median"), "{regressions:?}");
    }

    #[test]
    fn a_change_within_the_threshold_passes() {
        let (_, regressions) = compare(&report(11.0, 1), &report(10.0, 1), 0.15);
        assert!(regressions.is_empty(), "{regressions:?}");
    }

    #[test]
    fn recomputing_more_is_a_regression_even_when_faster() {
        let (_, regressions) = compare(&report(1.0, 5), &report(10.0, 1), 0.15);
        assert_eq!(regressions.len(), 1);
        assert!(
            regressions[0].detail.contains("recomputed 5"),
            "{regressions:?}"
        );
    }

    #[test]
    fn a_non_steady_measurement_fails_even_when_latency_and_recomputes_match() {
        let mut run = report(10.0, 1);
        run.measurements[0].steady_state = false;

        let (text, regressions) = compare(&run, &report(10.0, 1), 0.15);

        assert_eq!(regressions.len(), 1, "{regressions:?}");
        assert!(
            regressions[0]
                .detail
                .contains("measurement is not steady-state"),
            "{regressions:?}"
        );
        assert!(text.contains("REGRESSION"), "{text}");
    }

    #[test]
    fn a_non_steady_baseline_cannot_legitimise_mixed_work() {
        let mut baseline = report(10.0, 1);
        baseline.measurements[0].steady_state = false;

        let (text, regressions) = compare(&report(10.0, 1), &baseline, 0.15);

        assert_eq!(regressions.len(), 1, "{regressions:?}");
        assert!(
            regressions[0]
                .detail
                .contains("baseline is not steady-state"),
            "{regressions:?}"
        );
        assert!(text.contains("REGRESSION"), "{text}");
    }

    #[test]
    fn a_new_measurement_is_reported_without_failing_an_existing_baseline() {
        let mut run = report(1.0, 1);
        run.measurements
            .push(measurement("small/new-thing", 1.0, 1));

        let (text, regressions) = compare(&run, &report(1.0, 1), 0.15);

        assert!(regressions.is_empty());
        assert!(text.contains("new, not in baseline"), "{text}");
    }

    #[test]
    fn a_baseline_measurement_missing_from_the_run_is_a_regression() {
        let run = Report::new("test");

        let (text, regressions) = compare(&run, &report(1.0, 1), 0.15);

        assert_eq!(regressions.len(), 1, "{regressions:?}");
        assert_eq!(regressions[0].id, "small/warm-impl-edit");
        assert!(
            regressions[0]
                .detail
                .contains("missing from the current run"),
            "{regressions:?}"
        );
        assert!(text.contains("in baseline but not measured"), "{text}");
        assert!(text.contains("REGRESSION"), "{text}");
    }

    #[test]
    fn cache_hit_rate_is_validated_over_every_query_touched() {
        let s = DbStats {
            parse_computed: 1,
            interface_computed: 1,
            project_computed: 0,
            validated: 8,
            ..DbStats::default()
        };
        assert_eq!(cache_hit_rate(&s), 0.8);
        assert_eq!(cache_hit_rate(&DbStats::default()), 0.0);
    }

    #[test]
    fn report_round_trips_through_json() {
        let r = report(1.5, 2);
        let text = serde_json::to_string(&r).expect("serialise");
        let back: Report = serde_json::from_str(&text).expect("deserialise");
        assert_eq!(back.measurements[0].id, r.measurements[0].id);
        assert_eq!(back.measurements[0].median_ms, 1.5);
        assert_eq!(back.schema, SCHEMA_VERSION);
    }

    #[test]
    fn linked_rows_are_labelled_and_explain_fresh_graph_traversal() {
        let mut r = Report::new("linked graph warm iteration");
        r.workloads.push(WorkloadInfo {
            name: "small".to_string(),
            modules: 3,
            layers: 2,
            import_edges: 2,
            source_lines: 12,
            edit_target: "src/app/main.ts".to_string(),
            edit_target_dependents: 1,
        });
        let mut linked = measurement("small/warm-linked-body-edit", 2.0, 1);
        linked.operation = "linked graph body edit".to_string();
        r.measurements.push(linked);

        let text = render_human(&r);
        assert!(text.contains("linked interpreter"), "{text}");
        assert!(text.contains("fresh entry-rooted graph"), "{text}");
        assert!(text.contains("memoized per-module module_ir"), "{text}");
        assert!(
            !text.contains("no module-to-module edges"),
            "stale graph narrative leaked into report: {text}"
        );
    }

    #[test]
    fn measurement_kind_distinguishes_scalar_linked_and_native_work() {
        let scalar = measurement("small/warm-execute", 1.0, 1);
        assert_eq!(measurement_kind(&scalar), "scalar interpreter");

        let mut linked = measurement("small/warm-linked-noop", 1.0, 0);
        linked.operation = "linked interpreter no-op".to_string();
        assert_eq!(measurement_kind(&linked), "linked interpreter");

        let mut native = measurement("native/graph-warm-build", 1.0, 0);
        native.workload = "native".to_string();
        native.native = Some(NativeArtifactInfo {
            cache_hit: true,
            generated_c_bytes: 1,
            object_bytes: 1,
            executable_bytes: 1,
        });
        assert_eq!(measurement_kind(&native), "native linked graph");
    }
}
