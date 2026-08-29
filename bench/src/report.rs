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
        }
    }

    fn recomputes(&self) -> u64 {
        self.stats.parse_computed + self.stats.interface_computed + self.stats.project_computed
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
    let recomputed = s.parse_computed + s.interface_computed + s.project_computed;
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
        interface_computed: s.interface_computed,
        project_computed: s.project_computed,
        validated: s.validated,
        early_cutoff: s.early_cutoff,
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

    for info in &report.workloads {
        render_workload(&mut out, report, info);
    }
    out.push_str(NOTES);
    out
}

/// Everything a reader needs in order not to over-read the table above.
const NOTES: &str = "\
notes
  \"daemon startup\" is measured in-process: engine construction plus ingesting every source,
  deriving nothing. kiln-daemon has no process to spawn yet, so binary load and the socket
  handshake are not in that number and it is a floor, not the real figure.

  \"cache hit rate\" is validated / (validated + recomputed) over every memo the pipeline
  touched, so a query validated on the way down a dependency walk counts as a hit.

  v0's query graph is parse -> interface -> project with no module-to-module edges, so blast
  radius is currently uniform and the edited module's dependent count does not yet change any
  number above. The workload import graph is real so that it will when those edges land.

  §13's warm budget is an edit reaching the interpreter. This pipeline is a prefix of that, so
  passing it is necessary and nowhere near sufficient; the headroom will be spent by items 4
  and 5 of the implementation order.

";

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

    let _ = writeln!(
        out,
        "  {:<24} {:>10} {:>10} {:>10} {:>10} {:>10} {:>6} {:>8}",
        "operation", "median", "min", "p90", "max", "goal", "iters", "vs goal"
    );
    let _ = writeln!(out, "  {}", "-".repeat(92));
    for m in &mine {
        let goal = m.goal_ms.map_or_else(|| "-".to_string(), ms);
        let _ = writeln!(
            out,
            "  {:<24} {:>10} {:>10} {:>10} {:>10} {:>10} {:>6} {:>8}",
            m.operation,
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
        "  {:<24} {:>7} {:>10} {:>8} {:>10} {:>13} {:>9}",
        "operation", "parse", "interface", "project", "validated", "early cutoff", "hit rate"
    );
    let _ = writeln!(out, "  {}", "-".repeat(92));
    for m in &mine {
        let _ = writeln!(
            out,
            "  {:<24} {:>7} {:>10} {:>8} {:>10} {:>13} {:>8.1}%",
            m.operation,
            m.stats.parse_computed,
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
/// Two independent gates. Wall clock has a threshold and a noise floor because it is a noisy
/// measurement; recompute counts have neither, because they are exact and an edit that starts
/// recomputing more work has regressed no matter what the clock says.
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

        let _ = writeln!(
            out,
            "  {:<34} {:>10} vs {:>10}  {:>+7.1}%   recompute {} vs {}   {}",
            m.id,
            ms(m.median_ms),
            ms(base.median_ms),
            pct * 100.0,
            recomputed,
            base_recomputed,
            if slower || recomputed > base_recomputed {
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
    }

    for base in &baseline.measurements {
        if run.get(&base.id).is_none() {
            let _ = writeln!(
                out,
                "  {:<34} in baseline but not measured this run",
                base.id
            );
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
    fn measurements_missing_from_either_side_are_reported_but_do_not_fail() {
        let mut run = report(1.0, 1);
        run.measurements[0].id = "small/new-thing".to_string();
        let (text, regressions) = compare(&run, &report(1.0, 1), 0.15);
        assert!(regressions.is_empty());
        assert!(text.contains("new, not in baseline"), "{text}");
        assert!(text.contains("in baseline but not measured"), "{text}");
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
}
