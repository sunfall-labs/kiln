//! The measured operations from design.md §13, for the subset that exists in v0.
//!
//! Everything here is written against the [`QueryEngine`] trait rather than a concrete backend,
//! so the same harness can be pointed at a second implementation (docs/adr/0002) without being
//! rewritten. `new_engine` is the only line that names the backend.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use kiln_artifact_cache::{
    ArtifactCache, ArtifactKey, ArtifactKind, Backend, BuildProfile as CacheProfile, Digest, Lookup,
};
use kiln_backend_c::emit_c;
use kiln_db::{FileId, MemoEngine, QueryEngine, Stats as DbStats};
use kiln_native_runtime::{launcher_source, runtime_source};
use kiln_native_toolchain::{
    BuildOptions, CompileOptions, CompileUnit, LinkOptions, Toolchain, ToolchainError,
};

use crate::report::{Measurement, NativeArtifactInfo, Scenario};
use crate::timing::Samples;
use crate::workload::{EditKind, Workload};

fn new_engine() -> impl QueryEngine {
    MemoEngine::new(frontend_oxc::parse)
}

/// What one warm iteration does. Named here because the human report prints it: a latency
/// number is meaningless without saying which work it covers.
pub const WARM_PIPELINE: &str =
    "warm edit -> memoized parse/IR/interface queries -> project or interpreter";

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
const NATIVE_LINK_WARMUP: usize = 1;
const NATIVE_LINK_ITERS: usize = 5;

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
        warm_execution(w, budget)?,
        warm(w, EditKind::Interface, budget)?,
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

/// Measure the native seam on a tiny scalar module. Cold builds intentionally run only a few
/// times: this is a toolchain smoke measurement, not a reason to burn minutes recompiling C.
/// Once one build has populated the layered cache, the warm scenario repeatedly validates every
/// generated-C/object/executable entry without invoking the compiler.
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

    let program = native_program()?;
    let module = emit_c(&program).map_err(|error| format!("emitting native fixture: {error}"))?;
    let runtime = runtime_source().to_owned();
    let launcher = launcher_source(&module.symbol)
        .map_err(|error| format!("emitting native launcher: {error}"))?;
    let root = native_temp_dir();
    fs::create_dir_all(&root).map_err(|error| format!("creating {}: {error}", root.display()))?;
    let cache = ArtifactCache::new(root.join("cache"));
    let source_digest = Digest::hash(module.source.as_bytes());
    let runtime_digest = Digest::hash(runtime.as_bytes());
    let launcher_digest = Digest::hash(launcher.as_bytes());
    let compiler = toolchain.identity();
    let module_key = native_key(ArtifactKind::GeneratedC, compiler, [source_digest]);
    let runtime_key = native_key(ArtifactKind::GeneratedC, compiler, [runtime_digest]);
    let launcher_key = native_key(ArtifactKind::GeneratedC, compiler, [launcher_digest]);
    let module_object_key = native_key(ArtifactKind::NativeObject, compiler, [source_digest]);
    let runtime_object_key = native_key(ArtifactKind::NativeObject, compiler, [runtime_digest]);
    let launcher_object_key = native_key(ArtifactKind::NativeObject, compiler, [launcher_digest]);

    let (cold_warmup, cold, build) = native_cold_builds(
        &toolchain,
        &root,
        &module.source,
        &runtime,
        &launcher,
        budget,
    )?;
    let object_digests = build
        .objects
        .iter()
        .map(|object| {
            fs::read(&object.output_path)
                .map(|bytes| Digest::hash(&bytes))
                .map_err(|error| format!("reading {}: {error}", object.output_path.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let executable_bytes = fs::read(&build.executable.output_path).map_err(|error| {
        format!(
            "reading {}: {error}",
            build.executable.output_path.display()
        )
    })?;
    let executable_key = native_key(ArtifactKind::LinkedArtifact, compiler, object_digests);
    cache
        .put(&module_key, module.source.as_bytes())
        .map_err(|error| format!("caching generated C: {error}"))?;
    cache
        .put(&runtime_key, runtime.as_bytes())
        .map_err(|error| format!("caching runtime C: {error}"))?;
    cache
        .put(&launcher_key, launcher.as_bytes())
        .map_err(|error| format!("caching launcher C: {error}"))?;
    for (key, object) in [
        (&module_object_key, &build.objects[0]),
        (&runtime_object_key, &build.objects[1]),
        (&launcher_object_key, &build.objects[2]),
    ] {
        let bytes = fs::read(&object.output_path)
            .map_err(|error| format!("reading {}: {error}", object.output_path.display()))?;
        cache
            .put(key, &bytes)
            .map_err(|error| format!("caching native object: {error}"))?;
    }
    cache
        .put(&executable_key, &executable_bytes)
        .map_err(|error| format!("caching linked executable: {error}"))?;

    let sizes = NativeArtifactInfo {
        cache_hit: false,
        generated_c_bytes: module.source.len() as u64,
        object_bytes: build.objects.iter().map(|object| object.output_size).sum(),
        executable_bytes: build.executable.output_size,
    };
    let mut measurements = vec![native_measurement(
        "cold-build",
        "cold native C/object/link build",
        cold_warmup,
        cold,
        sizes.clone(),
    )];

    let (cache_warmup, warm) = native_cache_hits(
        &cache,
        budget,
        [
            &module_key,
            &runtime_key,
            &launcher_key,
            &module_object_key,
            &runtime_object_key,
            &launcher_object_key,
            &executable_key,
        ],
    )?;
    measurements.push(native_measurement(
        "warm-cache-build",
        "warm native cache-hit build",
        cache_warmup,
        warm,
        NativeArtifactInfo {
            cache_hit: true,
            ..sizes.clone()
        },
    ));

    let (link_warmup, link) = native_link_latency(&toolchain, &root, &build, budget)?;
    measurements.push(native_measurement(
        "link",
        "native link latency",
        link_warmup,
        link,
        sizes.clone(),
    ));

    let equivalence = native_equivalence(&build.executable.output_path, &program)?;
    measurements.push(native_measurement(
        "equivalence",
        "interpreter/native stdout equivalence",
        0,
        vec![equivalence],
        NativeArtifactInfo {
            cache_hit: true,
            ..sizes
        },
    ));
    let _ = fs::remove_dir_all(&root);
    Ok(Some(measurements))
}

fn native_program() -> Result<kiln_ir::Program, String> {
    let source = "let total: number = 0;\nwhile (total < 3) total += 1;\nconst state = total === 3 ? \"ready\" : \"wrong\";\nconsole.log(\"kiln\", state, total);\n";
    let mut engine = MemoEngine::new(frontend_oxc::parse);
    let file = engine.intern("examples/hello.ts");
    engine.set_source(file, source.to_owned());
    engine
        .ir(file)
        .map_err(|diagnostics| format!("compiling native fixture: {diagnostics:?}"))
}

fn native_cold_builds(
    toolchain: &Toolchain,
    root: &Path,
    module: &str,
    runtime: &str,
    launcher: &str,
    budget: Budget,
) -> Result<(usize, Vec<Duration>, kiln_native_toolchain::BuildReport), String> {
    let (warmup, iterations) = budget.apply(NATIVE_COLD_WARMUP, NATIVE_COLD_ITERS);
    let mut durations = Vec::with_capacity(iterations);
    let mut first_build = None;
    for index in 0..warmup + iterations {
        let dir = root.join(format!("cold-{index}"));
        fs::create_dir_all(&dir).map_err(|error| format!("creating {}: {error}", dir.display()))?;
        let module_source = dir.join("module.c");
        let runtime_source_path = dir.join("runtime.c");
        let launcher_source_path = dir.join("launcher.c");
        fs::write(&module_source, module)
            .and_then(|_| fs::write(&runtime_source_path, runtime))
            .and_then(|_| fs::write(&launcher_source_path, launcher))
            .map_err(|error| format!("writing native sources: {error}"))?;
        let units = vec![
            CompileUnit::new(&module_source, dir.join("module.o")),
            CompileUnit::new(&runtime_source_path, dir.join("runtime.o")),
            CompileUnit::new(&launcher_source_path, dir.join("launcher.o")),
        ];
        let executable = dir.join("hello");
        let started = Instant::now();
        let build = toolchain
            .build(&units, &executable, &native_build_options())
            .map_err(|error| format!("native cold build: {error}"))?;
        let elapsed = started.elapsed();
        if index >= warmup {
            durations.push(elapsed);
        }
        if first_build.is_none() {
            first_build = Some(build);
        }
    }
    Ok((
        warmup,
        durations,
        first_build.expect("native build iterations always include one warmup or sample"),
    ))
}

fn native_cache_hits(
    cache: &ArtifactCache,
    budget: Budget,
    keys: [&ArtifactKey; 7],
) -> Result<(usize, Vec<Duration>), String> {
    let (warmup, iterations) = budget.apply(NATIVE_CACHE_WARMUP, NATIVE_CACHE_ITERS);
    let mut durations = Vec::with_capacity(iterations);
    for index in 0..warmup + iterations {
        let started = Instant::now();
        for key in keys {
            if !matches!(cache.lookup(key), Ok(Lookup::Hit(_))) {
                return Err(format!("native cache key {} was not a hit", key.digest()));
            }
        }
        if index >= warmup {
            durations.push(started.elapsed());
        }
    }
    Ok((warmup, durations))
}

fn native_link_latency(
    toolchain: &Toolchain,
    root: &Path,
    build: &kiln_native_toolchain::BuildReport,
    budget: Budget,
) -> Result<(usize, Vec<Duration>), String> {
    let (warmup, iterations) = budget.apply(NATIVE_LINK_WARMUP, NATIVE_LINK_ITERS);
    let objects = build
        .objects
        .iter()
        .map(|object| object.output_path.clone())
        .collect::<Vec<_>>();
    let mut durations = Vec::with_capacity(iterations);
    for index in 0..warmup + iterations {
        let executable = root.join(format!("link-{index}"));
        let started = Instant::now();
        toolchain
            .link_executable(&objects, &executable, &native_link_options())
            .map_err(|error| format!("native link: {error}"))?;
        if index >= warmup {
            durations.push(started.elapsed());
        }
    }
    Ok((warmup, durations))
}

fn native_equivalence(executable: &Path, program: &kiln_ir::Program) -> Result<Duration, String> {
    let mut host = kiln_runtime::CaptureHost::new();
    let started = Instant::now();
    kiln_interpreter::execute(program, &mut host, Default::default())
        .map_err(|error| format!("interpreter equivalence run: {error}"))?;
    let native = Command::new(executable)
        .output()
        .map_err(|error| format!("native equivalence run: {error}"))?;
    if !native.status.success() {
        return Err(format!(
            "native equivalence process failed with {:?}: {}",
            native.status.code(),
            String::from_utf8_lossy(&native.stderr)
        ));
    }
    let native_stdout = String::from_utf8_lossy(&native.stdout);
    if native_stdout != host.stdout() {
        return Err(format!(
            "interpreter/native stdout mismatch: interpreter {:?}, native {:?}",
            host.stdout(),
            native_stdout
        ));
    }
    Ok(started.elapsed())
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

fn native_build_options() -> BuildOptions {
    BuildOptions {
        compile: CompileOptions::default(),
        link: native_link_options(),
    }
}

fn native_link_options() -> LinkOptions {
    LinkOptions {
        libraries: if cfg!(windows) {
            Vec::new()
        } else {
            vec!["m".to_owned()]
        },
        ..LinkOptions::default()
    }
}

fn native_key(
    kind: ArtifactKind,
    compiler: &kiln_native_toolchain::ToolIdentity,
    inputs: impl IntoIterator<Item = Digest>,
) -> ArtifactKey {
    ArtifactKey::new(
        kind,
        compiler.stable_key(),
        Backend::C,
        format!(
            "kiln-native-runtime-{}",
            kiln_native_runtime::NATIVE_RUNTIME_VERSION
        ),
        format!(
            "{}:{}-{}",
            compiler.target,
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
        "default",
        CacheProfile::Debug,
        "none",
        Digest::hash(b"kiln-bench-native-v1"),
        inputs,
    )
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
        kiln_interpreter::execute(&program, &mut host, Default::default())
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
