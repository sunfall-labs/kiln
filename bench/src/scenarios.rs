//! The measured operations from design.md §13, for the subset that exists in v0.
//!
//! Everything here is written against the [`QueryEngine`] trait rather than a concrete backend,
//! so the same harness can be pointed at a second implementation (docs/adr/0002) without being
//! rewritten. `new_engine` is the only line that names the backend.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use kiln_db::{FileId, MemoEngine, QueryEngine, Stats as DbStats};
use kiln_ir::{
    BYTECODE_VERSION, Constant, ExportBinding, HostOp, ImportBinding, Instruction, ModuleProgram,
    Program,
};
use kiln_module_graph::ModuleGraph;
use kiln_native::{NativeBuildOptions, NativeBuilder, NativeProfile};
use kiln_native_toolchain::{Toolchain, ToolchainError};

use crate::report::{Measurement, NativeArtifactInfo, Scenario};
use crate::timing::Samples;
use crate::workload::{EditKind, Workload};

fn new_engine() -> impl QueryEngine {
    MemoEngine::new(frontend_oxc::parse)
}

/// What one warm iteration does. Named here because the human report prints it: a latency
/// number is meaningless without saying which work it covers.
pub const WARM_PIPELINE: &str =
    "warm edit -> memoized queries -> project, scalar interpreter, or fresh linked graph";

/// Iteration budgets. Warm edits are cheap and noisy, so they get more samples; the cold
/// scenarios rebuild the whole project every iteration and get fewer.
const STARTUP_WARMUP: usize = 3;
const STARTUP_ITERS: usize = 12;
const COLD_WARMUP: usize = 2;
const COLD_ITERS: usize = 10;
const WARM_WARMUP: usize = 5;
const WARM_ITERS: usize = 25;
const NATIVE_COLD_WARMUP: usize = 1;
const NATIVE_COLD_ITERS: usize = 3;
const NATIVE_CACHE_WARMUP: usize = 2;
const NATIVE_CACHE_ITERS: usize = 15;

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

pub fn run_all(
    w: &Workload,
    budget: Budget,
    examples_dir: &Path,
) -> Result<Vec<Measurement>, String> {
    Ok(vec![
        startup(w, budget),
        cold(w, budget),
        warm(w, EditKind::Implementation, budget)?,
        warm_execution(w, budget)?,
        warm(w, EditKind::Interface, budget)?,
        warm_linked_execution(w, examples_dir, budget)?,
    ])
}

/// Targeted Test262 latency for the current parser/interpreter slice. The sources are parsed
/// once as Test262 metadata, then every sample measures variant selection through execution.
pub fn targeted_conformance(budget: Budget) -> Result<Measurement, String> {
    let (warmup, iters) = budget.apply(WARM_WARMUP, WARM_ITERS);
    let cases = [
        kiln_test262::parse(
            "targeted/raw.js",
            "/*---\nflags: [raw]\n---*/\nlet n = 1; while (n < 4) n += 1;\n",
        )
        .map_err(|error| error.to_string())?,
        kiln_test262::parse(
            "targeted/negative.js",
            "/*---\nflags: [raw]\nnegative:\n  phase: parse\n  type: SyntaxError\n---*/\nlet broken = ;\n",
        )
        .map_err(|error| error.to_string())?,
    ];
    let mut durations = Vec::with_capacity(iters);
    for iteration in 0..warmup + iters {
        let started = Instant::now();
        for case in &cases {
            for variant in
                kiln_conformance::run_case_variants(case, kiln_conformance::RunMode::Interpreter)
            {
                if matches!(variant.result.outcome, kiln_conformance::Outcome::Fail) {
                    return Err(format!(
                        "targeted conformance fixture failed: {:?}",
                        variant.result.diagnostics
                    ));
                }
            }
        }
        if iteration >= warmup {
            durations.push(started.elapsed());
        }
    }
    Ok(Measurement::new(
        Scenario {
            workload: "conformance",
            slug: "targeted-test262",
            operation: "targeted Test262 parser/interpreter batch",
            goal_ms: None,
        },
        warmup,
        Samples::from(&durations),
        DbStats::default(),
        true,
    ))
}

/// Measure the real native linked-graph seam on two scalar modules. The graph is rebuilt for every
/// sample so the operation includes graph construction as well as `NativeBuilder::build_graph`.
/// Cold samples use isolated caches; warm samples prove whole-graph reuse and one-module
/// invalidation. The fixture deliberately supplies precompiled scalar module IR with no functions
/// or closures, isolating graph/native-cache cost from the source-to-IR work measured elsewhere.
pub fn native(budget: Budget) -> Result<Option<Vec<Measurement>>, String> {
    let toolchain = match Toolchain::discover() {
        Ok(toolchain) => toolchain,
        Err(ToolchainError::NoCompiler { searched }) => {
            eprintln!(
                "kiln-bench: native measurements skipped: no supported C compiler found (searched: {})",
                searched.join(", ")
            );
            return Ok(None);
        }
        Err(error) => return Err(format!("discovering native compiler: {error}")),
    };
    let mut measurements = Vec::with_capacity(4);
    measurements.push(native_cold_graph(&toolchain, budget)?);
    let (warm, body_edit, graph, report, _cleanup) = native_warm_graph(&toolchain, budget)?;
    measurements.push(warm);
    measurements.push(body_edit);
    let equivalence = native_equivalence(&graph, &report)?;
    measurements.push(equivalence);
    Ok(Some(measurements))
}

fn native_cold_graph(toolchain: &Toolchain, budget: Budget) -> Result<Measurement, String> {
    let (warmup, iterations) = budget.apply(NATIVE_COLD_WARMUP, NATIVE_COLD_ITERS);
    let mut durations = Vec::with_capacity(iterations);
    let mut sizes = None;
    for index in 0..warmup + iterations {
        let root = native_temp_dir();
        let _cleanup = TempTree(root.clone());
        native_fixture_files(&root)?;
        let builder = NativeBuilder::new(
            kiln_artifact_cache::ArtifactCache::new(root.join("cache")),
            toolchain.clone(),
        );
        let (_, report, elapsed) = timed_native_graph(&builder, &root, 42, index)?;
        assert_cold_report(&report)?;
        sizes = Some(native_sizes(&report));
        if index >= warmup {
            durations.push(elapsed);
        }
    }
    Ok(native_measurement(
        "graph-cold-build",
        "cold linked graph native build",
        warmup,
        durations,
        sizes.expect("native cold loop always has one iteration"),
    ))
}

fn native_warm_graph(
    toolchain: &Toolchain,
    budget: Budget,
) -> Result<
    (
        Measurement,
        Measurement,
        ModuleGraph,
        kiln_native::NativeBuildReport,
        TempTree,
    ),
    String,
> {
    let root = native_temp_dir();
    let cleanup = TempTree(root.clone());
    native_fixture_files(&root)?;
    let builder = NativeBuilder::new(
        kiln_artifact_cache::ArtifactCache::new(root.join("cache")),
        toolchain.clone(),
    );
    let (graph, initial, _) = timed_native_graph(&builder, &root, 42, 0)?;
    assert_cold_report(&initial)?;
    let (warmup, iterations) = budget.apply(NATIVE_CACHE_WARMUP, NATIVE_CACHE_ITERS);
    let mut warm_durations = Vec::with_capacity(iterations);
    let mut warm_sizes = native_sizes(&initial);
    let mut warm_report = None;
    for index in 0..warmup + iterations {
        let (_, report, elapsed) = timed_native_graph(&builder, &root, 42, index + 1)?;
        assert_warm_report(&report)?;
        warm_sizes = native_sizes(&report);
        warm_report = Some(report);
        if index >= warmup {
            warm_durations.push(elapsed);
        }
    }

    let (edit_warmup, edit_iterations) = budget.apply(NATIVE_CACHE_WARMUP, NATIVE_CACHE_ITERS);
    let mut edit_durations = Vec::with_capacity(edit_iterations);
    let mut edit_sizes = warm_sizes.clone();
    for index in 0..edit_warmup + edit_iterations {
        let value = 1000 + index as u64;
        let (_, report, elapsed) = timed_native_graph(&builder, &root, value, index + 1)?;
        assert_body_edit_report(&report)?;
        edit_sizes = native_sizes(&report);
        if index >= edit_warmup {
            edit_durations.push(elapsed);
        }
    }
    let warm_report = warm_report.expect("native warm loop always has one iteration");
    Ok((
        native_measurement(
            "graph-warm-build",
            "warm unchanged linked graph build",
            warmup,
            warm_durations,
            NativeArtifactInfo {
                cache_hit: true,
                ..warm_sizes
            },
        ),
        native_measurement(
            "graph-body-edit",
            "precompiled body-edit native rebuild",
            edit_warmup,
            edit_durations,
            NativeArtifactInfo {
                cache_hit: false,
                ..edit_sizes
            },
        ),
        graph,
        warm_report,
        cleanup,
    ))
}

fn timed_native_graph(
    builder: &NativeBuilder,
    root: &Path,
    dependency_value: u64,
    output_index: usize,
) -> Result<(ModuleGraph, kiln_native::NativeBuildReport, Duration), String> {
    let started = Instant::now();
    let graph = native_graph(root, dependency_value)?;
    let output = root.join(format!("output-{dependency_value}-{output_index}"));
    let report = builder
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output,
                profile: NativeProfile::Dev,
            },
        )
        .map_err(|error| format!("native linked graph build: {error}"))?;
    Ok((graph, report, started.elapsed()))
}

fn native_graph(root: &Path, dependency_value: u64) -> Result<ModuleGraph, String> {
    let entry = root.join("main.js");
    let dependency = root.join("dep.js");
    let entry_program = native_entry_program();
    let dependency_program = native_dependency_program(dependency_value);
    let canonical_dependency = dependency
        .canonicalize()
        .map_err(|error| format!("canonicalizing {}: {error}", dependency.display()))?;
    ModuleGraph::build_with_modules(entry, root, move |path| {
        if path == canonical_dependency.as_path() {
            dependency_program.clone()
        } else {
            entry_program.clone()
        }
    })
    .map_err(|error| format!("constructing native module graph: {error}"))
}

fn native_fixture_files(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| format!("creating {}: {error}", root.display()))?;
    fs::write(root.join("dep.js"), "export const value = 42;\n")
        .and_then(|_| fs::write(root.join("main.js"), "import { value } from './dep.js';\n"))
        .map_err(|error| format!("writing native graph fixture: {error}"))
}

fn native_dependency_program(value: u64) -> ModuleProgram {
    ModuleProgram {
        program: Program {
            version: BYTECODE_VERSION,
            constants: vec![Constant::Number((value as f64).to_bits())],
            code: vec![
                Instruction::Const(0),
                Instruction::StoreLocal(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: vec![ExportBinding {
            exported: "value".to_owned(),
            local_slot: 0,
        }],
    }
}

fn native_entry_program() -> ModuleProgram {
    ModuleProgram {
        program: Program {
            version: BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![
                Instruction::LoadCell(0),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
        },
        dependencies: vec!["./dep.js".to_owned()],
        imports: vec![ImportBinding {
            source: "./dep.js".to_owned(),
            imported: "value".to_owned(),
            local: "value".to_owned(),
            local_slot: 0,
        }],
        exports: Vec::new(),
    }
}

fn native_sizes(report: &kiln_native::NativeBuildReport) -> NativeArtifactInfo {
    NativeArtifactInfo {
        cache_hit: report.cache_hits.executable,
        generated_c_bytes: report.sizes.generated_c,
        object_bytes: report.sizes.application_object
            + report.sizes.runtime_object
            + report.sizes.launcher_object,
        executable_bytes: report.sizes.executable,
    }
}

fn assert_cold_report(report: &kiln_native::NativeBuildReport) -> Result<(), String> {
    if report.module_artifacts.len() != 2
        || report
            .module_artifacts
            .iter()
            .any(|module| module.generated_c_cache_hit || module.object_cache_hit)
        || report.runtime_generated_c_cache_hit
        || report.launcher_generated_c_cache_hit
        || report.cache_hits.runtime_object
        || report.cache_hits.launcher_object
        || report.cache_hits.executable
    {
        return Err(format!(
            "native cold graph unexpectedly reused artifacts: {report:?}"
        ));
    }
    Ok(())
}

fn assert_warm_report(report: &kiln_native::NativeBuildReport) -> Result<(), String> {
    if report.module_artifacts.len() != 2
        || report
            .module_artifacts
            .iter()
            .any(|module| !module.generated_c_cache_hit || !module.object_cache_hit)
        || !report.runtime_generated_c_cache_hit
        || !report.launcher_generated_c_cache_hit
        || !report.cache_hits.generated_c
        || !report.cache_hits.application_object
        || !report.cache_hits.runtime_object
        || !report.cache_hits.launcher_object
        || !report.cache_hits.executable
    {
        return Err(format!("native warm graph missed an artifact: {report:?}"));
    }
    Ok(())
}

fn assert_body_edit_report(report: &kiln_native::NativeBuildReport) -> Result<(), String> {
    let misses = report
        .module_artifacts
        .iter()
        .filter(|module| !module.generated_c_cache_hit)
        .count();
    let object_misses = report
        .module_artifacts
        .iter()
        .filter(|module| !module.object_cache_hit)
        .count();
    let dependency = report
        .module_artifacts
        .iter()
        .find(|module| module.logical_path.as_deref() == Some("dep.js"));
    let sibling = report
        .module_artifacts
        .iter()
        .find(|module| module.logical_path.as_deref() == Some("main.js"));
    if misses != 1
        || object_misses != 1
        || report.cache_hits.generated_c
        || report.cache_hits.application_object
        || dependency.is_none_or(|module| module.generated_c_cache_hit || module.object_cache_hit)
        || sibling.is_none_or(|module| !module.generated_c_cache_hit || !module.object_cache_hit)
        || !report.runtime_generated_c_cache_hit
        || !report.launcher_generated_c_cache_hit
        || !report.cache_hits.runtime_object
        || !report.cache_hits.launcher_object
        || report.cache_hits.executable
    {
        return Err(format!(
            "native body edit invalidated the wrong artifacts: {report:?}"
        ));
    }
    Ok(())
}

fn native_equivalence(
    graph: &ModuleGraph,
    report: &kiln_native::NativeBuildReport,
) -> Result<Measurement, String> {
    let started = Instant::now();
    let mut host = kiln_runtime::CaptureHost::new();
    graph
        .execute(&mut host, Default::default())
        .map_err(|error| format!("interpreter graph equivalence run: {error}"))?;
    let native = Command::new(&report.output)
        .output()
        .map_err(|error| format!("native graph equivalence run: {error}"))?;
    if !native.status.success() || !native.stderr.is_empty() {
        return Err(format!(
            "native graph equivalence process failed with {:?}: {}",
            native.status.code(),
            String::from_utf8_lossy(&native.stderr)
        ));
    }
    let expected = host.stdout().as_bytes();
    if native.stdout != expected {
        return Err(format!(
            "interpreter/native graph stdout mismatch: interpreter {:?}, native {:?}",
            host.stdout(),
            String::from_utf8_lossy(&native.stdout)
        ));
    }
    Ok(native_measurement(
        "graph-equivalence",
        "linked interpreter/native stdout equivalence",
        0,
        vec![started.elapsed()],
        NativeArtifactInfo {
            cache_hit: report.cache_hits.executable,
            ..native_sizes(report)
        },
    ))
}

fn native_measurement(
    slug: &'static str,
    operation: &'static str,
    warmup: usize,
    durations: Vec<Duration>,
    native: NativeArtifactInfo,
) -> Measurement {
    let mut measurement = Measurement::new(
        Scenario {
            workload: "native",
            slug,
            operation,
            goal_ms: None,
        },
        warmup,
        Samples::from(&durations),
        DbStats::default(),
        true,
    );
    measurement.native = Some(native);
    measurement
}

fn native_temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "kiln-bench-native-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ))
}

struct TempTree(PathBuf);

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
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

/// The compiler-user loop: edit a small TypeScript program, rebuild its bytecode in the warm
/// database, and execute it immediately without launching a native toolchain.
fn warm_execution(w: &Workload, budget: Budget) -> Result<Measurement, String> {
    let (warmup, iters) = budget.apply(WARM_WARMUP, WARM_ITERS);
    let mut e = new_engine();
    let file = e.intern(&format!("{}/warm-execution.ts", w.spec.name));
    e.set_source(file, execution_source(0));
    e.ir(file)
        .map_err(|diagnostics| format!("priming interpreter scenario failed: {diagnostics:?}"))?;

    let mut acc = Accumulator::new(warmup, iters);
    for i in 0..warmup + iters {
        let seed = 1 + (i % 2) as u32;
        e.reset_stats();
        let started = Instant::now();
        e.set_source(file, execution_source(seed));
        let program = e.ir(file).map_err(|diagnostics| {
            format!("compiling interpreter scenario failed: {diagnostics:?}")
        })?;
        let mut host = kiln_runtime::CaptureHost::new();
        kiln_interpreter::execute(std::rc::Rc::new(program), &mut host, Default::default())
            .map_err(|error| format!("executing interpreter scenario failed: {error}"))?;
        let elapsed = started.elapsed();
        let expected = format!("warm {}\n", 100 + seed);
        if host.stdout() != expected {
            return Err(format!(
                "interpreter scenario produced {:?}, expected {expected:?}",
                host.stdout()
            ));
        }
        acc.push(elapsed, e.stats());
    }

    Ok(acc.finish(
        w,
        "warm-execute",
        "warm interpreted execution",
        Some(w.spec.warm_goal_ms),
    ))
}

/// The linked compiler-user loop: edit one scalar module, rebuild the entry-rooted module graph
/// from memoized module IR, and execute every linked module. The synthetic entry imports every
/// generated module, so this measures the full deterministic graph rather than one random branch.
fn warm_linked_execution(
    w: &Workload,
    examples_dir: &Path,
    budget: Budget,
) -> Result<Measurement, String> {
    let (warmup, iters) = budget.apply(WARM_WARMUP, WARM_ITERS);
    let root = fs::canonicalize(examples_dir.join(w.spec.name).join(w.linked_dir()))
        .map_err(|error| format!("locating materialized workload {}: {error}", w.spec.name))?;
    let paths = linked_paths(w, &root);
    let mut ids = Vec::with_capacity(paths.len());
    let mut e = new_engine();
    let mut texts = w.linked_base_texts.clone();
    texts.push(w.entry_text());
    load(&mut e, &paths, texts, &mut ids);

    let entry = root.join(w.entry_path());
    let (graph, provider_calls) = build_linked_graph(&mut e, &entry, &root)?;
    let accounting = LinkedGraphAccounting::of(&graph, provider_calls);
    validate_linked_graph_accounting(w, accounting)?;
    let stdout = execute_linked(&graph)?;
    if stdout != "linked 0\n" {
        return Err(format!(
            "workload {} linked graph produced {stdout:?}, expected \"linked 0\\n\"",
            w.spec.name
        ));
    }

    let target = ids[w.edit_target];
    let target_path = root.join(w.modules[w.edit_target].path());
    let base_target_program = graph
        .module(&target_path)
        .ok_or_else(|| format!("linked graph omitted edit target {}", target_path.display()))?
        .program
        .clone();
    e.reset_stats();

    let mut acc = Accumulator::new(warmup, iters);
    for i in 0..warmup + iters {
        let text = w.linked_variant(w.edit_target, 1 + (i % 2) as u32);
        e.reset_stats();
        let started = Instant::now();
        e.set_source(target, text);
        let (graph, provider_calls) = build_linked_graph(&mut e, &entry, &root)?;
        validate_linked_graph_accounting(w, LinkedGraphAccounting::of(&graph, provider_calls))?;
        if graph
            .module(&target_path)
            .is_some_and(|module| module.program == base_target_program)
        {
            return Err(format!(
                "workload {} body edit did not change linked target IR",
                w.spec.name
            ));
        }
        let stdout = execute_linked(&graph)?;
        if stdout != "linked 0\n" {
            return Err(format!(
                "workload {} linked graph produced {stdout:?}, expected \"linked 0\\n\"",
                w.spec.name
            ));
        }
        acc.push(started.elapsed(), e.stats());
    }

    Ok(acc.finish(
        w,
        "warm-linked-execute",
        "linked interpreter body edit",
        None,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LinkedGraphAccounting {
    modules: usize,
    edges: usize,
    provider_calls: usize,
}

impl LinkedGraphAccounting {
    fn of(graph: &ModuleGraph, provider_calls: usize) -> Self {
        Self {
            modules: graph.module_count(),
            edges: graph
                .modules()
                .map(|module| module.dependencies.len())
                .sum(),
            provider_calls,
        }
    }
}

fn validate_linked_graph_accounting(
    w: &Workload,
    accounting: LinkedGraphAccounting,
) -> Result<(), String> {
    if accounting.modules != w.modules.len() + 1
        || accounting.edges != w.import_edges() + w.modules.len()
        || accounting.provider_calls != accounting.modules
    {
        return Err(format!(
            "workload {} linked graph accounting mismatch: {:?}",
            w.spec.name, accounting
        ));
    }
    Ok(())
}

fn linked_paths(w: &Workload, root: &Path) -> Vec<String> {
    w.modules
        .iter()
        .map(|module| root.join(module.path()).display().to_string())
        .chain(std::iter::once(
            root.join(w.entry_path()).display().to_string(),
        ))
        .collect()
}

fn build_linked_graph(
    e: &mut impl QueryEngine,
    entry: &Path,
    project_root: &Path,
) -> Result<(ModuleGraph, usize), String> {
    let mut provider_calls = 0;
    let graph = ModuleGraph::build_with_module_provider(entry, project_root, |module_path| {
        provider_calls += 1;
        let module_path_string = module_path.to_str().ok_or_else(|| {
            kiln_module_graph::ModuleGraphError::FrontendPath {
                path: module_path.to_path_buf(),
            }
        })?;
        let file = e.intern(module_path_string);
        if e.source(file).is_none() {
            return Err(kiln_module_graph::ModuleGraphError::Source {
                path: module_path.to_path_buf(),
                message: "source is not synchronized".to_owned(),
            });
        }
        e.module_ir(file)
            .map_err(|diagnostics| kiln_module_graph::ModuleGraphError::Compile {
                path: module_path.to_path_buf(),
                diagnostics,
            })
    })
    .map_err(|error| format!("building linked graph: {error}"))?;
    Ok((graph, provider_calls))
}

fn execute_linked(graph: &ModuleGraph) -> Result<String, String> {
    let mut host = kiln_runtime::CaptureHost::new();
    graph
        .execute(&mut host, Default::default())
        .map_err(|error| format!("executing linked graph: {error}"))?;
    Ok(host.into_stdout())
}

fn execution_source(seed: u32) -> String {
    format!(
        "let n: number = 0;\nwhile (n < 100) n += 1;\nconst seed = {seed};\nconsole.log('warm', n + seed);\n"
    )
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

fn recompute_counters(s: &DbStats) -> [u64; 6] {
    [
        s.parse_computed,
        s.ir_computed,
        s.interface_computed,
        s.project_computed,
        s.validated,
        s.early_cutoff,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workload::{SMALL, Workload};

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "kiln-bench-linked-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ))
    }

    #[test]
    fn linked_small_scenario_rebuilds_one_edited_module_and_emits_stable_output() {
        let root = temp_root();
        let _cleanup = TempTree(root.clone());
        let workload = Workload::generate(&SMALL);
        workload
            .materialise(&root, true)
            .expect("materialise linked fixture");

        let measurement = warm_linked_execution(
            &workload,
            &root,
            Budget {
                warmup: Some(0),
                iterations: Some(1),
            },
        )
        .expect("linked scenario");

        assert_eq!(measurement.id, "small/warm-linked-execute");
        assert_eq!(measurement.stats.parse_computed, 1);
        assert_eq!(measurement.stats.ir_computed, 1);
        assert_eq!(measurement.stats.project_computed, 0);
        assert!(measurement.steady_state);
    }

    #[test]
    fn native_entry_fixture_reads_import_cells_before_host_calls() {
        let root = temp_root();
        let _cleanup = TempTree(root.clone());
        native_fixture_files(&root).expect("materialise native fixture");
        let graph = native_graph(&root, 42).expect("construct native graph");

        assert_eq!(
            execute_linked(&graph).expect("execute native fixture"),
            "42\n"
        );
    }
}
