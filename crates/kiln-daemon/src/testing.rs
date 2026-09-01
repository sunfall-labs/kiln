//! Resident conformance coordination: discover once, select deterministically, then run the
//! selected batch without launching a compiler process per test.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Instant;

use kiln_conformance::{
    CaseResult, ConformanceTest, InterpreterWorker, Outcome, Provenance, ReasonReport, RunMode,
    VariantResult,
};
use kiln_proto::{
    Diagnostic, TestCaseSummary, TestMode, TestOutcome, TestReasonSummary, TestSummary,
};
use kiln_test_selector::{ChangeSet, FailureState, TestId, TestMetadata, TestTier, select};
use rayon::prelude::*;

const MODULE_GRAPH_FEATURES: &[&str] = &["modules", "module-graph"];
const MODULE_RESOLVER_FEATURES: &[&str] = &["builtin-modules", "node-packages", "node-resolution"];
const MAX_CACHED_CORPORA: usize = 8;

thread_local! {
    /// One non-`Send` runtime per persistent Rayon thread.
    ///
    /// Rayon owns work stealing and the process-lifetime threads; this slot owns only the
    /// current-thread interpreter scheduler. A case never enters this slot with another case's
    /// VM, globals, host capture, or values.
    static INTERPRETER_WORKER: RefCell<Option<InterpreterWorker>> = const { RefCell::new(None) };
}

/// Process-lifetime conformance inventory and per-corpus singleflight owned by one resident daemon.
///
/// A changed-test request caused by compiler/runtime code can reuse the same parsed frontmatter,
/// manifest, and Test262 harness. Changes that may alter the corpus deliberately rediscover it;
/// a request without change provenance does too, because the daemon cannot prove the files stayed
/// unchanged. Equal cold requests share one discovery, while independent corpora never hold one
/// another behind filesystem I/O. Execution remains per-request and every case still creates a
/// fresh realm.
pub(crate) struct ConformanceCoordinator {
    catalog: Mutex<ConformanceCatalog>,
    discoverer: Arc<DiscoverCorpus>,
}

#[derive(Default)]
struct ConformanceCatalog {
    clock: u64,
    corpora: BTreeMap<PathBuf, CachedCorpus>,
    discovering: BTreeMap<PathBuf, Arc<InFlightDiscovery>>,
}

type DiscoverCorpus = dyn Fn(&Path) -> Result<Vec<ConformanceTest>, String> + Send + Sync;
type DiscoveryResult = Result<Arc<Vec<ConformanceTest>>, String>;

struct InFlightDiscovery {
    provenance: DiscoveryProvenance,
    result: Mutex<Option<DiscoveryResult>>,
    ready: Condvar,
}

impl InFlightDiscovery {
    fn new(provenance: DiscoveryProvenance) -> Self {
        Self {
            provenance,
            result: Mutex::new(None),
            ready: Condvar::new(),
        }
    }

    fn wait(&self) -> DiscoveryResult {
        let mut result = self.result.lock().unwrap_or_else(PoisonError::into_inner);
        while result.is_none() {
            result = self
                .ready
                .wait(result)
                .unwrap_or_else(PoisonError::into_inner);
        }
        result
            .as_ref()
            .expect("discovery result is present after wait")
            .clone()
    }

    fn publish(&self, result: DiscoveryResult) {
        let mut slot = self.result.lock().unwrap_or_else(PoisonError::into_inner);
        debug_assert!(slot.is_none(), "one owner publishes one discovery result");
        *slot = Some(result);
        self.ready.notify_all();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiscoveryProvenance {
    changed_root: Option<PathBuf>,
    changed_paths: Option<Vec<String>>,
}

impl DiscoveryProvenance {
    fn new(changed_root: Option<&Path>, changed_paths: Option<&[String]>) -> Self {
        let changed_root = changed_root.and_then(|root| root.canonicalize().ok());
        let changed_paths = changed_paths.map(|paths| {
            let mut paths = paths
                .iter()
                .map(|path| normalize_path(Path::new(path)))
                .collect::<Vec<_>>();
            paths.sort();
            paths.dedup();
            paths
        });
        Self {
            changed_root,
            changed_paths,
        }
    }

    fn proves_unchanged(&self, root: &Path, tests: &[ConformanceTest]) -> bool {
        let (Some(changed_root), Some(changed_paths)) =
            (self.changed_root.as_deref(), self.changed_paths.as_deref())
        else {
            return false;
        };
        root.starts_with(changed_root)
            && !corpus_inputs_changed(root, changed_root, tests, changed_paths)
    }
}

struct CachedCorpus {
    tests: Arc<Vec<ConformanceTest>>,
    last_used: u64,
}

struct CatalogLookup {
    tests: Arc<Vec<ConformanceTest>>,
    cache_hit: bool,
}

impl Default for ConformanceCoordinator {
    fn default() -> Self {
        Self {
            catalog: Mutex::new(ConformanceCatalog::default()),
            discoverer: Arc::new(|root| {
                kiln_conformance::discover(root).map_err(|error| error.to_string())
            }),
        }
    }
}

impl ConformanceCoordinator {
    #[cfg(test)]
    fn with_discoverer(
        discoverer: impl Fn(&Path) -> Result<Vec<ConformanceTest>, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            catalog: Mutex::new(ConformanceCatalog::default()),
            discoverer: Arc::new(discoverer),
        }
    }

    pub(crate) fn run(
        &self,
        root: &str,
        mode: TestMode,
        changed_root: Option<String>,
        changed_paths: Option<Vec<String>>,
        previously_failed: Vec<String>,
    ) -> Result<TestSummary, String> {
        let root = Path::new(root);
        let discovered = self.discover(
            root,
            changed_root.as_deref().map(Path::new),
            changed_paths.as_deref(),
        )?;
        run_discovered(
            discovered.tests.as_slice(),
            mode,
            changed_paths,
            previously_failed,
            discovered.cache_hit,
        )
    }

    fn discover(
        &self,
        root: &Path,
        changed_root: Option<&Path>,
        changed_paths: Option<&[String]>,
    ) -> Result<CatalogLookup, String> {
        let key = root.canonicalize().map_err(|error| {
            format!(
                "canonicalizing conformance root {}: {error}",
                root.display()
            )
        })?;
        let provenance = DiscoveryProvenance::new(changed_root, changed_paths);
        loop {
            let (discovery, owns_discovery) = {
                let mut catalog = self.catalog.lock().unwrap_or_else(PoisonError::into_inner);
                catalog.clock = catalog.clock.wrapping_add(1);
                let now = catalog.clock;

                // A refresh already in flight takes precedence over the previous ready value.
                // Waiters later compare provenance before accepting its result, so a change that
                // arrived after this discovery began cannot be hidden by singleflight.
                if let Some(discovery) = catalog.discovering.get(&key) {
                    (Arc::clone(discovery), false)
                } else if let Some(cached) = catalog.corpora.get_mut(&key)
                    && provenance.proves_unchanged(&key, &cached.tests)
                {
                    cached.last_used = now;
                    return Ok(CatalogLookup {
                        tests: Arc::clone(&cached.tests),
                        cache_hit: true,
                    });
                } else {
                    // An invalid ready value is not a fallback: if refresh fails, the next request
                    // must retry instead of silently reusing metadata already proven stale.
                    catalog.corpora.remove(&key);
                    let discovery = Arc::new(InFlightDiscovery::new(provenance.clone()));
                    catalog
                        .discovering
                        .insert(key.clone(), Arc::clone(&discovery));
                    (discovery, true)
                }
            };

            if !owns_discovery {
                let tests = discovery.wait()?;
                if discovery.provenance == provenance || provenance.proves_unchanged(&key, &tests) {
                    return Ok(CatalogLookup {
                        tests,
                        cache_hit: false,
                    });
                }
                // This request names corpus inputs the completed discovery did not cover. The
                // owner removes its in-flight marker before publishing, so looping elects exactly
                // one follow-up owner and lets equivalent waiters join it.
                continue;
            }

            // Filesystem traversal and frontmatter parsing deliberately happen outside the
            // catalogue mutex. The per-root entry above is the singleflight seam: equal roots
            // coalesce, while independent corpora no longer serialize their cold discovery.
            let result = catch_unwind(AssertUnwindSafe(|| (self.discoverer)(&key)))
                .map_err(|_| format!("conformance discovery panicked for {}", key.display()))
                .and_then(|result| result)
                .map(Arc::new);

            {
                let mut catalog = self.catalog.lock().unwrap_or_else(PoisonError::into_inner);
                catalog.discovering.remove(&key);
                if let Ok(discovered) = &result {
                    catalog.clock = catalog.clock.wrapping_add(1);
                    let now = catalog.clock;
                    catalog.corpora.insert(
                        key.clone(),
                        CachedCorpus {
                            tests: Arc::clone(discovered),
                            last_used: now,
                        },
                    );
                    while catalog.corpora.len() > MAX_CACHED_CORPORA {
                        let Some(oldest) = catalog
                            .corpora
                            .iter()
                            .min_by_key(|(path, cached)| (cached.last_used, *path))
                            .map(|(path, _)| path.clone())
                        else {
                            break;
                        };
                        catalog.corpora.remove(&oldest);
                    }
                }
                // Publish before removing the marker, under the same catalogue critical section.
                // No equal invalidating request can slip between ready-cache insertion and waiter
                // notification and accidentally elect a redundant owner.
                discovery.publish(result.clone());
                catalog.discovering.remove(&key);
            }
            return result.map(|tests| CatalogLookup {
                tests,
                cache_hit: false,
            });
        }
    }
}

#[cfg(test)]
pub fn run(
    root: &str,
    mode: TestMode,
    changed_paths: Option<Vec<String>>,
    previously_failed: Vec<String>,
) -> Result<TestSummary, String> {
    run_with_changed_root(root, mode, None, changed_paths, previously_failed)
}

pub fn run_with_changed_root(
    root: &str,
    mode: TestMode,
    changed_root: Option<String>,
    changed_paths: Option<Vec<String>>,
    previously_failed: Vec<String>,
) -> Result<TestSummary, String> {
    ConformanceCoordinator::default().run(
        root,
        mode,
        changed_root,
        changed_paths,
        previously_failed,
    )
}

fn run_discovered(
    discovered: &[ConformanceTest],
    mode: TestMode,
    changed_paths: Option<Vec<String>>,
    previously_failed: Vec<String>,
    discovery_cache_hit: bool,
) -> Result<TestSummary, String> {
    let started = Instant::now();
    let run_mode = match mode {
        TestMode::Parse => RunMode::Parse,
        TestMode::Interpreter => RunMode::Interpreter,
        TestMode::Node => RunMode::Node,
    };

    let mut tests_by_id = BTreeMap::new();
    let mut metadata = Vec::with_capacity(discovered.len());
    for test in discovered {
        metadata.push(selector_metadata(test, mode));
        tests_by_id.insert(test.id().to_owned(), test);
    }

    let changes = changed_paths
        .as_deref()
        .map(|paths| change_set_for_tests(paths, tests_by_id.values().copied()))
        .unwrap_or_else(ChangeSet::full_run);
    let failures =
        FailureState::from_failing(previously_failed.iter().map(|id| TestId::new(file_id(id))));
    let selection = select(metadata, &changes, &failures);

    let mut summary = TestSummary {
        discovered: tests_by_id.len(),
        discovery_cache_hit,
        ..TestSummary::default()
    };
    // Resolve the selection into a dense list once, in selection order. Every stage below indexes
    // into this list, which is what keeps thread completion order out of the reported summary:
    // the workers only ever answer "slot `i` produced these variants".
    let pending = selection
        .tests
        .iter()
        .filter_map(|selected| {
            tests_by_id
                .get(selected.id.as_str())
                .copied()
                .map(|test| (selected.id.to_string(), test))
        })
        .collect::<Vec<_>>();
    let executed = execute(&pending, run_mode)?;

    let mut results = Vec::new();
    for ((id, test), variants) in pending.iter().zip(executed) {
        for variant in variants {
            let variant_name = variant.variant.name();
            let case_id = match test.provenance() {
                // A manifest expectation records one observed invocation, so its stable manifest
                // ID already names the complete case. Realm suffixes distinguish only the
                // multiple executions required by Test262 frontmatter.
                Provenance::Manifest => id.clone(),
                Provenance::Frontmatter => format!("{id}#{variant_name}"),
            };
            let outcome = wire_outcome(variant.result.outcome);
            count(&mut summary, outcome);
            summary.cases.push(TestCaseSummary {
                id: case_id.clone(),
                path: id.clone(),
                variant: variant_name.to_owned(),
                outcome,
                elapsed_nanos: duration_nanos(variant.result.timing.elapsed),
                diagnostics: variant
                    .result
                    .diagnostics
                    .iter()
                    .map(|diagnostic| Diagnostic {
                        path: Some(id.clone()),
                        message: format!("{}: {}", diagnostic.phase.tag(), diagnostic.message),
                        start: diagnostic.span.map(|span| span.start),
                        end: diagnostic.span.map(|span| span.end),
                    })
                    .collect(),
            });
            results.push((case_id, variant.result));
        }
    }
    summary.reasons = reason_distribution(&results);
    summary.selected = summary.cases.len();
    // Wall clock, which no longer equals the summed case time now that cases run concurrently.
    // That is the intended reading: `docs/design.md` §13 budgets how long a developer waits for
    // the affected tests, not how much CPU the batch consumed. The per-case `elapsed_nanos` is
    // still each case's own duration, so the two no longer add up and are not meant to.
    summary.elapsed_millis = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    Ok(summary)
}

/// Run every selected test, returning each one's variants in the caller's order.
///
/// Rayon's indexed iterator steals uneven work while collecting results in selection order. Pure
/// interpreter cases reuse scheduler infrastructure attached to the Rayon thread, but build and
/// reclaim a fresh VM, realm, global object, host capture, and `Rc` graph for every case. Parse and
/// Node modes retain their existing execution paths; Node resource reset is a separate lifecycle
/// contract and must not be inferred from the safe pure-interpreter reuse here.
fn execute(
    pending: &[(String, &ConformanceTest)],
    mode: RunMode,
) -> Result<Vec<Vec<VariantResult>>, String> {
    match mode {
        RunMode::Interpreter => pending
            .par_iter()
            .map(|(_, test)| {
                with_interpreter_worker(|worker| test.run_with_interpreter_worker(worker))
            })
            .collect(),
        RunMode::Parse | RunMode::Node => pending
            .par_iter()
            .map(|(_, test)| {
                catch_unwind(AssertUnwindSafe(|| test.run(mode)))
                    .map_err(|_| "conformance worker panicked".to_owned())
            })
            .collect(),
    }
}

/// Run one operation on this thread's resident interpreter scheduler.
///
/// Construction happens inside the Rayon thread because the runtime is intentionally non-`Send`.
/// A panic drops the scheduler before the thread accepts another case. The conformance runner also
/// catches per-case panics to preserve a result; `is_reusable` carries interpreter-side unwinds
/// across that catch boundary so those cases retire the worker just as reliably.
fn with_interpreter_worker<T>(
    operation: impl FnOnce(&mut InterpreterWorker) -> T,
) -> Result<T, String> {
    INTERPRETER_WORKER.with(|slot| {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let mut worker = slot.borrow_mut();
            if worker.is_none() {
                *worker = Some(
                    InterpreterWorker::new()
                        .map_err(|error| format!("building conformance worker: {error}"))?,
                );
            }
            Ok(operation(
                worker
                    .as_mut()
                    .expect("interpreter worker is initialized above"),
            ))
        }));

        match outcome {
            Ok(result) => {
                if slot
                    .borrow()
                    .as_ref()
                    .is_some_and(|worker| !worker.is_reusable())
                {
                    slot.borrow_mut().take();
                }
                result
            }
            Err(_) => {
                slot.borrow_mut().take();
                Err("conformance worker panicked".to_owned())
            }
        }
    })
}

/// Group every non-passing result by reason, so the summary carries the backlog and not only its
/// size.  The grouping itself lives in `kiln-conformance` beside the runners that produce the
/// reasons; this only puts it on the wire.
fn reason_distribution(results: &[(String, CaseResult)]) -> Vec<TestReasonSummary> {
    ReasonReport::from_results(results.iter().map(|(id, result)| (id.as_str(), result)))
        .groups()
        .iter()
        .map(|group| TestReasonSummary {
            count: group.count(),
            outcome: wire_outcome(group.outcome),
            reason: group.reason.clone(),
        })
        .collect()
}

fn selector_metadata(test: &ConformanceTest, mode: TestMode) -> TestMetadata {
    let id = test.id();
    let mut metadata = TestMetadata::new(id)
        .with_tier(TestTier::Tier2)
        .compiler_crate("frontend-oxc")
        .compiler_crate("kiln-hir")
        .with_smoke(
            id.split('/').any(|component| component == "smoke")
                || test
                    .features()
                    .iter()
                    .any(|feature| feature == "kiln-smoke"),
        );
    if matches!(mode, TestMode::Interpreter | TestMode::Node) {
        for compiler_crate in ["kiln-ir", "kiln-interpreter", "kiln-runtime"] {
            metadata.add_compiler_crate(compiler_crate);
        }
        if has_any_feature(test, MODULE_GRAPH_FEATURES) {
            metadata.add_compiler_crate("kiln-module-graph");
        }
        if has_any_feature(test, MODULE_RESOLVER_FEATURES) {
            metadata.add_compiler_crate("kiln-module-resolver");
        }
    }
    for feature in test.features() {
        metadata.add_feature(feature);
        if let Some(module) = feature.strip_prefix("builtin-") {
            metadata.add_builtin_module(module);
        }
        if feature.starts_with("node:") {
            metadata.add_builtin_module(feature);
        }
        if feature == "node-http" {
            metadata.add_builtin_module("node:http");
        }
        if feature == "console-log" {
            metadata.add_host_op("console.log");
        }
    }
    metadata
}

fn has_any_feature(test: &ConformanceTest, features: &[&str]) -> bool {
    test.features()
        .iter()
        .any(|feature| features.contains(&feature.as_str()))
}

fn corpus_inputs_changed(
    root: &Path,
    changed_root: &Path,
    tests: &[ConformanceTest],
    paths: &[String],
) -> bool {
    let normalized_root = normalize_path(root);
    let repository_relative_root = root
        .strip_prefix(changed_root)
        .ok()
        .map(normalize_path)
        .unwrap_or_default();
    let test_paths = tests
        .iter()
        .map(|test| {
            (
                normalize_path(test.source_path()),
                normalize_path(test.path()),
                test.bundle_input_paths()
                    .iter()
                    .map(|path| normalize_path(path))
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();

    paths.iter().any(|path| {
        let normalized = normalize_path(Path::new(path));
        let components = normalized.split('/').collect::<Vec<_>>();
        let inside_absolute_root = path_is_within(&normalized, &normalized_root);
        let inside_repository_root = repository_relative_root.is_empty()
            || path_is_within(&normalized, &repository_relative_root);
        let manifest_changed = components.last() == Some(&"manifest.toml");
        let harness_changed = components.contains(&"harness")
            && Path::new(&normalized)
                .extension()
                .is_some_and(|extension| extension == "js");
        let known_input_changed =
            test_paths
                .iter()
                .any(|(source_path, program_path, bundle_input_paths)| {
                    path_matches(&normalized, source_path)
                        || path_matches(&normalized, program_path)
                        || bundle_input_paths
                            .iter()
                            .any(|input| path_matches(&normalized, input))
                });

        inside_absolute_root
            || inside_repository_root
            || manifest_changed
            || harness_changed
            || known_input_changed
    })
}

fn path_is_within(path: &str, root: &str) -> bool {
    !root.is_empty() && (path == root || path.starts_with(&format!("{root}/")))
}

fn change_set_for_tests<'a, I>(paths: &[String], tests: I) -> ChangeSet
where
    I: IntoIterator<Item = &'a ConformanceTest>,
{
    // These crates own the changed-test coordinator, discovery/runners, and selector itself. A
    // crate edit can therefore change the result for any discovered case, even when no test's
    // source or metadata names that crate. `kiln-proto` is deliberately not included: it carries
    // the Test request and summary over the wire, but has no selection or conformance semantics.
    const CONFORMANCE_SELECTION_CRATES: &[&str] =
        &["kiln-daemon", "kiln-conformance", "kiln-test-selector"];

    let mut changes = ChangeSet::new();
    let test_paths = tests
        .into_iter()
        .map(|test| {
            (
                normalize_path(test.source_path()),
                normalize_path(test.path()),
                test.bundle_input_paths()
                    .iter()
                    .map(|path| normalize_path(path))
                    .collect::<Vec<_>>(),
                TestId::new(test.id()),
            )
        })
        .collect::<Vec<_>>();
    for path in paths {
        let normalized = normalize_path(Path::new(path));
        let components = normalized.split('/').collect::<Vec<_>>();
        let owned_builtin = runtime_builtin_for_path(&normalized);
        if matches!(
            normalized.as_str(),
            "Cargo.toml" | "Cargo.lock" | "docs/design.md"
        ) {
            changes.force_full_run();
        }
        if normalized.rsplit('/').next() == Some("manifest.toml") {
            // The manifest owns corpus IDs, expectations, and metadata. A change anywhere in a
            // manifest can therefore affect every discovered test, even when no source path moved.
            changes.force_full_run();
        }
        if components.first() == Some(&"crates")
            && let Some(compiler_crate) = components.get(1)
        {
            if owned_builtin.is_none() {
                changes.add_compiler_crate(*compiler_crate);
            }
            if CONFORMANCE_SELECTION_CRATES.contains(compiler_crate) {
                changes.force_full_run();
            }
        }
        if let Some(module) = owned_builtin {
            changes.add_builtin_module(module);
        }
        if let Some(index) = components
            .iter()
            .position(|component| *component == "features")
            && let Some(feature) = components.get(index + 1)
        {
            changes.add_feature(feature.trim_end_matches(".js"));
        }
        if let Some(index) = components
            .iter()
            .position(|component| *component == "builtins")
            && let Some(module) = components.get(index + 1)
        {
            changes.add_builtin_module(module.trim_end_matches(".js"));
        }
        if normalized.contains("host-ops") || normalized.contains("host_ops") {
            changes.add_host_op("console.log");
        }
        for (source_path, program_path, bundle_input_paths, test_id) in &test_paths {
            if path_matches(&normalized, source_path)
                || path_matches(&normalized, program_path)
                || bundle_input_paths
                    .iter()
                    .any(|input| path_matches(&normalized, input))
            {
                changes.add_changed_test(test_id.clone());
            }
        }
    }
    changes
}

fn runtime_builtin_for_path(path: &str) -> Option<&'static str> {
    const OWNED_BUILTINS: &[(&str, &str)] = &[
        ("crates/kiln-runtime/src/node_http.rs", "node:http"),
        ("crates/kiln-runtime/src/node_net.rs", "node:net"),
    ];
    OWNED_BUILTINS.iter().find_map(|(source, module)| {
        path.strip_suffix(source)
            .is_some_and(|prefix| prefix.is_empty() || prefix.ends_with('/'))
            .then_some(*module)
    })
}

/// Normalize a repository path lexically. Changed paths can come from git on a different host,
/// so this deliberately does not canonicalize or access the filesystem (a deleted path is still a
/// valid change). Leading roots are ignored because matching is by repository-relative suffix.
fn normalize_path(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            component => components.push(component),
        }
    }
    components.join("/")
}

fn path_matches(changed: &str, test_path: &str) -> bool {
    !changed.is_empty() && (test_path == changed || test_path.ends_with(&format!("/{changed}")))
}

fn file_id(id: &str) -> &str {
    id.rsplit_once('#').map_or(id, |(path, _)| path)
}

fn wire_outcome(outcome: Outcome) -> TestOutcome {
    match outcome {
        Outcome::Pass => TestOutcome::Pass,
        Outcome::Fail => TestOutcome::Fail,
        Outcome::Unsupported => TestOutcome::Unsupported,
        Outcome::Skipped => TestOutcome::Skipped,
    }
}

fn count(summary: &mut TestSummary, outcome: TestOutcome) {
    match outcome {
        TestOutcome::Pass => summary.passed += 1,
        TestOutcome::Fail => summary.failed += 1,
        TestOutcome::Unsupported => summary.unsupported += 1,
        TestOutcome::Skipped => summary.skipped += 1,
    }
}

fn duration_nanos(duration: std::time::Duration) -> u64 {
    duration.as_nanos().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    static TEMP_CORPUS_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TemporaryCorpus {
        path: PathBuf,
    }

    impl TemporaryCorpus {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "kiln-changed-bundle-test-{}-{}",
                std::process::id(),
                TEMP_CORPUS_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("effect/src")).expect("create authored source directory");
            fs::create_dir_all(path.join("effect/bundle")).expect("create bundle directory");
            let source = b"console.log('source');\n";
            let bundle = b"console.log('bundle');\n";
            let package = b"{\"name\":\"fixture\"}\n";
            let lock = b"fixture lock\n";
            let build = b"#!/bin/sh\n";
            fs::write(path.join("effect/src/one.ts"), source).expect("write authored source");
            fs::write(path.join("effect/bundle/one.js"), bundle).expect("write executable bundle");
            fs::write(path.join("effect/package.json"), package).expect("write package metadata");
            fs::write(path.join("effect/package-lock.json"), lock).expect("write package lock");
            fs::write(path.join("effect/build.sh"), build).expect("write build script");
            let mut provenance = b"kiln-bundle-provenance-v2\n".to_vec();
            for input in [bundle.as_slice(), source, package, lock, build] {
                provenance.extend_from_slice(input.len().to_string().as_bytes());
                provenance.push(b'\n');
                provenance.extend_from_slice(input);
            }
            fs::write(path.join("effect/bundle/one.js.kiln-inputs"), provenance)
                .expect("write bundle provenance");
            fs::write(
                path.join("manifest.toml"),
                r#"[[test]]
id = "effect/one"
source = "effect/src/one.ts"
bundle = "effect/bundle/one.js"
bundle_inputs = ["effect/package.json", "effect/package-lock.json", "effect/build.sh"]
expected_stdout = "bundle\n"
"#,
            )
            .expect("write manifest");
            Self { path }
        }
    }

    impl Drop for TemporaryCorpus {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct RawFrontmatterCorpus {
        path: PathBuf,
    }

    impl RawFrontmatterCorpus {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "kiln-resident-worker-test-{}-{}",
                std::process::id(),
                TEMP_CORPUS_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create raw frontmatter corpus");
            fs::write(
                path.join("case.js"),
                "/*---\nflags: [raw]\n---*/\nconsole.log('resident');\n",
            )
            .expect("write raw frontmatter case");
            Self { path }
        }
    }

    impl Drop for RawFrontmatterCorpus {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn resident_interpreter_thread_reuses_its_runtime_and_recovers_after_a_panic() {
        let corpus = RawFrontmatterCorpus::new();
        let discovered = kiln_conformance::discover(&corpus.path).expect("discover raw case");
        let test = discovered.first().expect("one discovered case");
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .thread_name(|_| "kiln-resident-worker-test".to_owned())
            .build()
            .expect("build one-thread worker pool");

        let first = pool
            .install(|| with_interpreter_worker(|worker| test.run_with_interpreter_worker(worker)))
            .expect("first worker invocation");
        assert_eq!(first[0].result.outcome, Outcome::Pass);
        assert_eq!(first[0].result.stdout, "resident\n");
        assert_eq!(
            pool.install(|| { with_interpreter_worker(|worker| worker.completed_executions()) }),
            Ok(1),
            "the same thread retains its scheduler between jobs"
        );

        let panicked =
            pool.install(|| with_interpreter_worker::<()>(|_| panic!("injected worker failure")));
        assert_eq!(panicked, Err("conformance worker panicked".to_owned()));
        assert_eq!(
            pool.install(|| { with_interpreter_worker(|worker| worker.completed_executions()) }),
            Ok(0),
            "a panic retires the old runtime before the thread accepts more work"
        );

        let recovered = pool
            .install(|| with_interpreter_worker(|worker| test.run_with_interpreter_worker(worker)))
            .expect("replacement worker invocation");
        assert_eq!(recovered[0].result.outcome, Outcome::Pass);
        assert_eq!(
            pool.install(|| { with_interpreter_worker(|worker| worker.completed_executions()) }),
            Ok(1)
        );
    }

    #[test]
    fn resident_catalog_reuses_compiler_only_runs_and_refreshes_corpus_inputs() {
        let corpus = TemporaryCorpus::new();
        let root = corpus.path.to_string_lossy().into_owned();
        let changed_root_path = corpus.path.parent().expect("temporary corpus parent");
        let changed_root = changed_root_path.to_string_lossy().into_owned();
        let corpus_relative = corpus
            .path
            .file_name()
            .expect("temporary corpus name")
            .to_string_lossy();
        let coordinator = ConformanceCoordinator::default();
        let compiler_change = Some(vec!["crates/kiln-ir/src/lib.rs".to_owned()]);

        let cold = coordinator
            .run(
                &root,
                TestMode::Parse,
                Some(changed_root.clone()),
                compiler_change.clone(),
                Vec::new(),
            )
            .expect("cold discovery succeeds");
        assert!(!cold.discovery_cache_hit);

        let warm = coordinator
            .run(
                &root,
                TestMode::Parse,
                Some(changed_root.clone()),
                compiler_change.clone(),
                Vec::new(),
            )
            .expect("compiler-only rerun reuses discovery");
        assert!(warm.discovery_cache_hit);

        let source_edit = coordinator
            .run(
                &root,
                TestMode::Parse,
                Some(changed_root.clone()),
                Some(vec![format!("{corpus_relative}/effect/src/one.ts")]),
                Vec::new(),
            )
            .expect("authored source edit refreshes discovery");
        assert!(!source_edit.discovery_cache_hit);

        let manifest_edit = coordinator
            .run(
                &root,
                TestMode::Parse,
                Some(changed_root.clone()),
                Some(vec![format!("{corpus_relative}/manifest.toml")]),
                Vec::new(),
            )
            .expect("manifest edit refreshes discovery");
        assert!(!manifest_edit.discovery_cache_hit);

        let discovered = kiln_conformance::discover(&corpus.path).expect("fixture discovery");
        assert!(corpus_inputs_changed(
            &corpus.path,
            changed_root_path,
            &discovered,
            &["test262/harness/assert.js".to_owned()]
        ));
        assert!(corpus_inputs_changed(
            &corpus.path,
            changed_root_path,
            &discovered,
            &[corpus
                .path
                .join("new-case.js")
                .to_string_lossy()
                .into_owned()]
        ));
    }

    #[test]
    fn external_corpus_never_reuses_project_change_provenance() {
        let external = TemporaryCorpus::new();
        let project = TemporaryCorpus::new();
        let root = external.path.to_string_lossy().into_owned();
        let changed_root = project.path.to_string_lossy().into_owned();
        let coordinator = ConformanceCoordinator::default();
        let compiler_change = Some(vec!["crates/kiln-ir/src/lib.rs".to_owned()]);

        for _ in 0..2 {
            let summary = coordinator
                .run(
                    &root,
                    TestMode::Parse,
                    Some(changed_root.clone()),
                    compiler_change.clone(),
                    Vec::new(),
                )
                .expect("external corpus discovery succeeds");
            assert!(
                !summary.discovery_cache_hit,
                "project-relative paths cannot prove an external corpus unchanged"
            );
        }
    }

    #[test]
    fn resident_catalog_has_a_fixed_lru_bound() {
        let corpora = (0..=MAX_CACHED_CORPORA)
            .map(|_| TemporaryCorpus::new())
            .collect::<Vec<_>>();
        let coordinator = ConformanceCoordinator::default();
        for corpus in &corpora {
            coordinator
                .discover(
                    &corpus.path,
                    corpus.path.parent(),
                    Some(&["crates/kiln-ir/src/lib.rs".to_owned()]),
                )
                .expect("cache corpus");
        }

        let catalog = coordinator
            .catalog
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assert_eq!(catalog.corpora.len(), MAX_CACHED_CORPORA);
        assert!(
            !catalog.corpora.contains_key(
                &corpora[0]
                    .path
                    .canonicalize()
                    .expect("canonical first root")
            ),
            "the least recently used corpus is evicted"
        );
    }

    #[test]
    fn cold_discovery_of_independent_corpora_runs_concurrently() {
        let first = TemporaryCorpus::new();
        let second = TemporaryCorpus::new();
        let (started_tx, started_rx) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let discover_release = Arc::clone(&release);
        let coordinator = Arc::new(ConformanceCoordinator::with_discoverer(move |root| {
            started_tx
                .send(root.to_owned())
                .expect("test observes discovery start");
            let (released, ready) = &*discover_release;
            let mut released = released.lock().unwrap_or_else(PoisonError::into_inner);
            while !*released {
                released = ready.wait(released).unwrap_or_else(PoisonError::into_inner);
            }
            kiln_conformance::discover(root).map_err(|error| error.to_string())
        }));

        let handles = [&first, &second]
            .into_iter()
            .map(|corpus| {
                let coordinator = Arc::clone(&coordinator);
                let root = corpus.path.clone();
                thread::spawn(move || {
                    coordinator.discover(
                        &root,
                        root.parent(),
                        Some(&["crates/kiln-ir/src/lib.rs".to_owned()]),
                    )
                })
            })
            .collect::<Vec<_>>();

        let first_started = started_rx.recv_timeout(Duration::from_secs(2));
        let second_started = started_rx.recv_timeout(Duration::from_secs(2));
        {
            let (released, ready) = &*release;
            *released.lock().unwrap_or_else(PoisonError::into_inner) = true;
            ready.notify_all();
        }
        let results = handles
            .into_iter()
            .map(|handle| handle.join().expect("discovery thread does not panic"))
            .collect::<Vec<_>>();

        assert!(first_started.is_ok(), "first corpus starts discovery");
        assert!(
            second_started.is_ok(),
            "a different corpus must not wait on the catalogue mutex"
        );
        assert!(results.into_iter().all(|result| result.is_ok()));
    }

    #[test]
    fn equal_cold_discovery_is_singleflight_and_publishes_one_arc() {
        let corpus = TemporaryCorpus::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let discover_calls = Arc::clone(&calls);
        let (started_tx, started_rx) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let discover_release = Arc::clone(&release);
        let coordinator = Arc::new(ConformanceCoordinator::with_discoverer(move |root| {
            discover_calls.fetch_add(1, Ordering::SeqCst);
            started_tx.send(()).expect("test observes discovery start");
            let (released, ready) = &*discover_release;
            let mut released = released.lock().unwrap_or_else(PoisonError::into_inner);
            while !*released {
                released = ready.wait(released).unwrap_or_else(PoisonError::into_inner);
            }
            kiln_conformance::discover(root).map_err(|error| error.to_string())
        }));
        let changed_path = format!(
            "{}/effect/src/one.ts",
            corpus
                .path
                .file_name()
                .expect("temporary corpus name")
                .to_string_lossy()
        );

        let spawn_discovery = || {
            let coordinator = Arc::clone(&coordinator);
            let root = corpus.path.clone();
            let changed_path = changed_path.clone();
            thread::spawn(move || coordinator.discover(&root, root.parent(), Some(&[changed_path])))
        };
        let owner = spawn_discovery();
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("owner begins discovery");
        let waiter = spawn_discovery();

        let deadline = Instant::now() + Duration::from_secs(2);
        let waiter_joined = loop {
            let joined = {
                let catalog = coordinator
                    .catalog
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                catalog
                    .discovering
                    .values()
                    .next()
                    .is_some_and(|discovery| Arc::strong_count(discovery) >= 3)
            };
            if joined || Instant::now() >= deadline {
                break joined;
            }
            thread::yield_now();
        };
        assert!(waiter_joined, "second request joins the in-flight root");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        {
            let (released, ready) = &*release;
            *released.lock().unwrap_or_else(PoisonError::into_inner) = true;
            ready.notify_all();
        }
        let owner = owner
            .join()
            .expect("owner thread does not panic")
            .expect("owner discovery succeeds");
        let waiter = waiter
            .join()
            .expect("waiter thread does not panic")
            .expect("waiter discovery succeeds");
        assert!(!owner.cache_hit);
        assert!(!waiter.cache_hit);
        assert!(Arc::ptr_eq(&owner.tests, &waiter.tests));
        assert!(
            coordinator
                .catalog
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .discovering
                .is_empty()
        );
    }

    #[test]
    fn newer_corpus_change_waits_for_inflight_work_then_refreshes_once() {
        let corpus = TemporaryCorpus::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let discover_calls = Arc::clone(&calls);
        let (started_tx, started_rx) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let discover_release = Arc::clone(&release);
        let coordinator = Arc::new(ConformanceCoordinator::with_discoverer(move |root| {
            let call = discover_calls.fetch_add(1, Ordering::SeqCst);
            started_tx
                .send(call)
                .expect("test observes discovery start");
            if call == 0 {
                let (released, ready) = &*discover_release;
                let mut released = released.lock().unwrap_or_else(PoisonError::into_inner);
                while !*released {
                    released = ready.wait(released).unwrap_or_else(PoisonError::into_inner);
                }
            }
            kiln_conformance::discover(root).map_err(|error| error.to_string())
        }));
        let root = corpus.path.clone();
        let changed_root = root
            .parent()
            .expect("temporary corpus parent")
            .to_path_buf();
        let compiler_change = vec!["crates/kiln-ir/src/lib.rs".to_owned()];
        let source_change = vec![format!(
            "{}/effect/src/one.ts",
            root.file_name()
                .expect("temporary corpus name")
                .to_string_lossy()
        )];

        let owner = {
            let coordinator = Arc::clone(&coordinator);
            let root = root.clone();
            let changed_root = changed_root.clone();
            thread::spawn(move || {
                coordinator.discover(&root, Some(&changed_root), Some(&compiler_change))
            })
        };
        assert_eq!(
            started_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("first discovery starts"),
            0
        );
        let waiter = {
            let coordinator = Arc::clone(&coordinator);
            let root = root.clone();
            let changed_root = changed_root.clone();
            thread::spawn(move || {
                coordinator.discover(&root, Some(&changed_root), Some(&source_change))
            })
        };

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let joined = coordinator
                .catalog
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .discovering
                .values()
                .next()
                .is_some_and(|discovery| Arc::strong_count(discovery) >= 3);
            if joined {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "newer change first waits for the in-flight snapshot"
            );
            thread::yield_now();
        }
        {
            let (released, ready) = &*release;
            *released.lock().unwrap_or_else(PoisonError::into_inner) = true;
            ready.notify_all();
        }
        assert_eq!(
            started_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("newer corpus change starts one follow-up refresh"),
            1
        );

        let owner = owner
            .join()
            .expect("owner thread does not panic")
            .expect("first discovery succeeds");
        let waiter = waiter
            .join()
            .expect("waiter thread does not panic")
            .expect("follow-up discovery succeeds");
        assert!(!Arc::ptr_eq(&owner.tests, &waiter.tests));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_failed_refresh_removes_stale_metadata_and_the_next_request_retries() {
        let corpus = TemporaryCorpus::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let discover_calls = Arc::clone(&calls);
        let coordinator = ConformanceCoordinator::with_discoverer(move |root| {
            let call = discover_calls.fetch_add(1, Ordering::SeqCst);
            if call == 1 {
                return Err("injected discovery failure".to_owned());
            }
            kiln_conformance::discover(root).map_err(|error| error.to_string())
        });
        let compiler_change = ["crates/kiln-ir/src/lib.rs".to_owned()];
        let source_change = [format!(
            "{}/effect/src/one.ts",
            corpus
                .path
                .file_name()
                .expect("temporary corpus name")
                .to_string_lossy()
        )];

        let cold = coordinator
            .discover(&corpus.path, corpus.path.parent(), Some(&compiler_change))
            .expect("initial discovery succeeds");
        assert!(!cold.cache_hit);
        assert!(
            coordinator
                .discover(&corpus.path, corpus.path.parent(), Some(&source_change))
                .is_err()
        );
        let retry = coordinator
            .discover(&corpus.path, corpus.path.parent(), Some(&compiler_change))
            .expect("failed discovery is retried");
        assert!(!retry.cache_hit);
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        let warm = coordinator
            .discover(&corpus.path, corpus.path.parent(), Some(&compiler_change))
            .expect("successful retry becomes reusable");
        assert!(warm.cache_hit);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_panicking_discoverer_publishes_an_error_and_does_not_poison_retry() {
        let corpus = TemporaryCorpus::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let discover_calls = Arc::clone(&calls);
        let coordinator = ConformanceCoordinator::with_discoverer(move |root| {
            if discover_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("injected discovery panic");
            }
            kiln_conformance::discover(root).map_err(|error| error.to_string())
        });

        let error = match coordinator.discover(&corpus.path, None, None) {
            Err(error) => error,
            Ok(_) => panic!("discovery panic must become an ordinary coordinator error"),
        };
        assert!(error.contains("discovery panicked"), "{error}");
        assert!(
            coordinator
                .catalog
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .discovering
                .is_empty()
        );

        let retry = coordinator
            .discover(&corpus.path, None, None)
            .expect("next request retries after a panic");
        assert!(!retry.cache_hit);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn changed_crates_features_and_global_inputs_map_to_selector_tags() {
        let changes = change_set_for_tests(
            &[
                "crates/kiln-ir/src/lib.rs".to_owned(),
                "tests/features/BigInt.js".to_owned(),
            ],
            std::iter::empty::<&ConformanceTest>(),
        );
        assert!(changes.compiler_crates.contains("kiln-ir"));
        assert!(changes.features.contains("BigInt"));
        assert!(!changes.full_run);

        assert!(
            change_set_for_tests(
                &["Cargo.toml".to_owned()],
                std::iter::empty::<&ConformanceTest>()
            )
            .full_run
        );
    }

    #[test]
    fn changed_selection_crates_select_every_case_including_ordinary_tests() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let corpus = repo.join("conformance/guard");
        let discovered = kiln_conformance::discover(&corpus).expect("fixture corpus is valid");
        let metadata = discovered
            .iter()
            .map(|test| selector_metadata(test, TestMode::Parse))
            .collect::<Vec<_>>();
        let ordinary = metadata
            .iter()
            .find(|test| !test.smoke)
            .expect("guard corpus has an ordinary case");

        for crate_name in ["kiln-daemon", "kiln-conformance", "kiln-test-selector"] {
            let changed = format!("crates/{crate_name}/src/lib.rs");
            let changes = change_set_for_tests(&[changed], discovered.iter());
            assert!(
                changes.full_run,
                "{crate_name} must invalidate the whole corpus"
            );

            let selection = select(&metadata, &changes, &FailureState::new());
            assert_eq!(selection.len(), discovered.len(), "{crate_name}");
            assert!(
                selection
                    .tests
                    .iter()
                    .any(|selected| selected.id == ordinary.id),
                "{crate_name} must include ordinary case {}",
                ordinary.id
            );
        }

        // `kiln-proto` transports Test requests and summaries but does not participate in
        // discovery, execution, or selection, so a proto-only edit retains ordinary changed-test
        // behavior rather than broadening the run.
        let proto_changes = change_set_for_tests(
            &["crates/kiln-proto/src/lib.rs".to_owned()],
            discovered.iter(),
        );
        assert!(!proto_changes.full_run);
        assert!(proto_changes.compiler_crates.contains("kiln-proto"));
    }

    #[test]
    fn changed_module_graph_selects_module_cases_without_a_full_run() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let discovered =
            kiln_conformance::discover(&repo.join("conformance")).expect("fixture corpus is valid");
        let metadata = discovered
            .iter()
            .map(|test| selector_metadata(test, TestMode::Interpreter))
            .collect::<Vec<_>>();
        let expected = metadata
            .iter()
            .filter(|test| {
                test.features
                    .iter()
                    .any(|feature| MODULE_GRAPH_FEATURES.contains(&feature.as_str()))
            })
            .map(|test| test.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(!expected.is_empty(), "fixture corpus has module cases");

        let changes = change_set_for_tests(
            &["crates/kiln-module-graph/src/lib.rs".to_owned()],
            discovered.iter(),
        );
        assert!(!changes.full_run);
        let selection = select(&metadata, &changes, &FailureState::new());
        let selected = selection
            .ids()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(selected, expected);
        assert!(selection.tests.iter().all(|test| {
            test.has_reason(&kiln_test_selector::SelectionReason::ModifiedCompilerCrate(
                "kiln-module-graph".to_owned(),
            ))
        }));
    }

    #[test]
    fn changed_module_resolver_selects_resolution_cases_without_a_full_run() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let discovered =
            kiln_conformance::discover(&repo.join("conformance")).expect("fixture corpus is valid");
        let metadata = discovered
            .iter()
            .map(|test| selector_metadata(test, TestMode::Interpreter))
            .collect::<Vec<_>>();
        let expected = metadata
            .iter()
            .filter(|test| {
                test.features
                    .iter()
                    .any(|feature| MODULE_RESOLVER_FEATURES.contains(&feature.as_str()))
            })
            .map(|test| test.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(!expected.is_empty(), "fixture corpus has resolution cases");

        let changes = change_set_for_tests(
            &["crates/kiln-module-resolver/src/lib.rs".to_owned()],
            discovered.iter(),
        );
        assert!(!changes.full_run);
        let selection = select(&metadata, &changes, &FailureState::new());
        let selected = selection
            .ids()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(selected, expected);
        assert!(selection.tests.iter().all(|test| {
            test.has_reason(&kiln_test_selector::SelectionReason::ModifiedCompilerCrate(
                "kiln-module-resolver".to_owned(),
            ))
        }));
    }

    #[test]
    fn changed_node_http_runtime_selects_only_the_http_builtin_cases() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let discovered =
            kiln_conformance::discover(&repo.join("conformance")).expect("fixture corpus is valid");
        let metadata = discovered
            .iter()
            .map(|test| selector_metadata(test, TestMode::Node))
            .collect::<Vec<_>>();
        let expected = [
            "server/s01-http-hello",
            "server/s02-routing-json",
            "server/s03-request-body-streams",
            "server/s04-fastify",
            "server/s05-express",
        ]
        .into_iter()
        .map(TestId::new)
        .collect::<std::collections::BTreeSet<_>>();

        for changed in [
            "crates/kiln-runtime/src/node_http.rs",
            r"C:\checkout\kiln\crates\kiln-runtime\src\node_http.rs",
        ] {
            let changes = change_set_for_tests(&[changed.to_owned()], discovered.iter());
            assert!(!changes.full_run, "{changed}");
            assert!(changes.builtin_modules.contains("node:http"), "{changed}");
            assert!(
                !changes.compiler_crates.contains("kiln-runtime"),
                "{changed} should use the narrower builtin ownership tag"
            );

            let selection = select(&metadata, &changes, &FailureState::new());
            let selected = selection
                .ids()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(selected, expected, "{changed}");
            assert!(selection.tests.iter().all(|test| {
                test.has_reason(&kiln_test_selector::SelectionReason::ModifiedBuiltinModule(
                    "node:http".to_owned(),
                ))
            }));
        }

        for unrelated in [
            "crates/kiln-runtime/src/http_transport.rs",
            "crates/kiln-runtime/src/node_http_agent.rs",
        ] {
            let changes = change_set_for_tests(&[unrelated.to_owned()], discovered.iter());
            assert!(
                changes.compiler_crates.contains("kiln-runtime"),
                "{unrelated}"
            );
            assert!(changes.builtin_modules.is_empty(), "{unrelated}");
        }
    }

    #[test]
    fn changed_node_net_runtime_selects_only_the_net_builtin_cases() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let discovered = kiln_conformance::discover(&repo.join("conformance/node"))
            .expect("pinned Node corpus is valid");
        let metadata = discovered
            .iter()
            .map(|test| selector_metadata(test, TestMode::Node))
            .collect::<Vec<_>>();
        let expected = [
            "node-v24.7.0/net/listen-close-server",
            "node-v24.7.0/net/socket-set-encoding",
            "node-v24.7.0/http/tokio-owned-server-client",
            "node-v24.7.0/http/server-allow-half-open",
            "node-v24.7.0/http/pipelined-request-identity",
        ]
        .into_iter()
        .map(TestId::new)
        .collect::<std::collections::BTreeSet<_>>();

        for changed in [
            "crates/kiln-runtime/src/node_net.rs",
            r"C:\checkout\kiln\crates\kiln-runtime\src\node_net.rs",
        ] {
            let changes = change_set_for_tests(&[changed.to_owned()], discovered.iter());
            assert!(!changes.full_run, "{changed}");
            assert!(changes.builtin_modules.contains("node:net"), "{changed}");
            assert!(
                !changes.compiler_crates.contains("kiln-runtime"),
                "{changed} should use the narrower builtin ownership tag"
            );

            let selection = select(&metadata, &changes, &FailureState::new());
            let selected = selection
                .ids()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(selected, expected, "{changed}");
            assert!(selection.tests.iter().all(|test| {
                test.has_reason(&kiln_test_selector::SelectionReason::ModifiedBuiltinModule(
                    "node:net".to_owned(),
                ))
            }));
        }
    }

    /// The point of the index-keyed collection: the summary must not be able to observe which
    /// worker finished first. Running the same corpus repeatedly is the only honest check, since
    /// a single run of a scheduler that happens to stay ordered proves nothing.
    #[test]
    fn parallel_execution_reports_the_same_order_every_time() {
        let first = run_corpus("conformance");
        for _ in 0..4 {
            let again = run_corpus("conformance");
            assert_eq!(
                first.cases.iter().map(|case| &case.id).collect::<Vec<_>>(),
                again.cases.iter().map(|case| &case.id).collect::<Vec<_>>(),
                "case order is selection order, not completion order"
            );
            assert_eq!(first.reasons, again.reasons);
            assert_eq!(
                (first.passed, first.failed, first.unsupported, first.skipped),
                (again.passed, again.failed, again.unsupported, again.skipped)
            );
        }
    }

    #[test]
    fn persisted_variant_failures_select_their_source_file() {
        assert_eq!(file_id("language/a.js#strict"), "language/a.js");
        assert_eq!(file_id("language/a.js"), "language/a.js");
    }

    #[test]
    fn changed_conformance_path_selects_one_test_and_not_a_basename_prefix() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let corpus = repo.join("conformance/guard");
        let discovered = kiln_conformance::discover(&corpus).expect("fixture corpus is valid");
        let target = discovered
            .iter()
            .find(|test| test.id() == "guard/g01-spread-symbols")
            .expect("fixture target exists");
        let changed = target
            .path()
            .strip_prefix(&repo)
            .expect("fixture is under repository root")
            .to_string_lossy()
            .replace('\\', "/");

        let changes = change_set_for_tests(&[changed], discovered.iter());
        assert_eq!(changes.changed_tests.len(), 1);
        assert!(changes.changed_tests.contains(&TestId::new(target.id())));

        let windows_changed = target
            .path()
            .strip_prefix(&repo)
            .expect("fixture is under repository root")
            .to_string_lossy()
            .replace('/', "\\");
        let windows_changes = change_set_for_tests(&[windows_changed], discovered.iter());
        assert!(
            windows_changes
                .changed_tests
                .contains(&TestId::new(target.id()))
        );

        let selection = select(
            discovered
                .iter()
                .map(|test| selector_metadata(test, TestMode::Interpreter)),
            &changes,
            &FailureState::new(),
        );
        assert_eq!(selection.tests.len(), 1);
        assert_eq!(selection.tests[0].id, TestId::new(target.id()));

        let basename = target
            .path()
            .file_name()
            .expect("fixture target has a basename")
            .to_string_lossy();
        let prefixed = target.path().with_file_name(format!("not-{basename}"));
        assert!(!path_matches(basename.as_ref(), &normalize_path(&prefixed)));
    }

    #[test]
    fn changed_manifest_source_runs_only_its_stable_test_id() {
        let summary = run(
            &corpus("conformance/guard"),
            TestMode::Parse,
            Some(vec!["conformance/guard/g01-spread-symbols.js".to_owned()]),
            Vec::new(),
        )
        .expect("the changed manifest test is runnable");

        assert_eq!(summary.discovered, 20);
        assert_eq!(summary.selected, 1);
        assert_eq!(summary.cases.len(), 1);
        assert_eq!(summary.cases[0].path, "guard/g01-spread-symbols");
    }

    #[test]
    fn changed_manifest_source_selects_a_bundle_executed_case() {
        let corpus = TemporaryCorpus::new();
        let discovered = kiln_conformance::discover(&corpus.path).expect("discover bundle case");
        let test = &discovered[0];
        assert!(test.source_path().ends_with("effect/src/one.ts"));
        assert!(test.path().ends_with("effect/bundle/one.js"));

        let changes = change_set_for_tests(&["effect/src/one.ts".to_owned()], discovered.iter());
        assert_eq!(changes.changed_tests.len(), 1);
        assert!(changes.changed_tests.contains(&TestId::new("effect/one")));

        let selection = select(
            discovered
                .iter()
                .map(|test| selector_metadata(test, TestMode::Interpreter)),
            &changes,
            &FailureState::new(),
        );
        assert_eq!(selection.tests.len(), 1);
        assert_eq!(selection.tests[0].id, TestId::new("effect/one"));

        for build_input in [
            "effect/package.json",
            "effect/package-lock.json",
            "effect/build.sh",
        ] {
            let changes = change_set_for_tests(&[build_input.to_owned()], discovered.iter());
            assert!(
                changes.changed_tests.contains(&TestId::new("effect/one")),
                "changing shared bundle input {build_input} selects every dependent test"
            );
        }

        fs::write(
            corpus.path.join("effect/src/one.ts"),
            "console.log('edited source');\n",
        )
        .expect("edit authored source without rebuilding");
        let stale = kiln_conformance::discover(&corpus.path).expect("rediscover stale bundle");
        assert!(stale[0].path().ends_with("effect/src/one.ts"));
        let result = stale[0].run(kiln_conformance::RunMode::Parse);
        assert_eq!(result[0].result.outcome, Outcome::Skipped);
        assert!(
            result[0].result.diagnostics[0].message.contains("stale"),
            "a stale bundle is an explicit harness decision"
        );
    }

    #[test]
    fn changed_manifest_runs_every_guard_test() {
        let summary = run(
            &corpus("conformance/guard"),
            TestMode::Parse,
            Some(vec!["conformance/manifest.toml".to_owned()]),
            Vec::new(),
        )
        .expect("the manifest change is runnable");

        assert_eq!(summary.discovered, 20);
        assert_eq!(summary.selected, 20);
    }

    #[test]
    fn changed_baseline_does_not_select_ordinary_guard_tests() {
        let summary = run(
            &corpus("conformance/guard"),
            TestMode::Parse,
            Some(vec!["conformance/results/baseline.json".to_owned()]),
            Vec::new(),
        )
        .expect("the baseline change is runnable");

        assert_eq!(summary.discovered, 20);
        assert_eq!(summary.selected, 0);
        assert!(summary.cases.is_empty());
    }

    #[test]
    fn local_smoke_corpus_runs_as_one_classified_batch() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/smoke");
        let summary = run(
            root.to_str().expect("fixture path is UTF-8"),
            TestMode::Interpreter,
            None,
            Vec::new(),
        )
        .expect("local corpus is valid");

        assert_eq!(summary.discovered, 5);
        assert_eq!(summary.selected, 6, "ordinary scripts have two variants");
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.unsupported, 0);
        assert_eq!(summary.skipped, 2, "ordinary execution needs the harness");
        assert_eq!(summary.passed, 4);
    }

    fn corpus(relative: &str) -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative)
            .to_str()
            .expect("fixture path is UTF-8")
            .to_owned()
    }

    fn run_corpus(relative: &str) -> TestSummary {
        run(&corpus(relative), TestMode::Interpreter, None, Vec::new())
            .expect("the corpus is discoverable")
    }

    #[test]
    fn the_manifest_corpus_runs_and_every_non_pass_keeps_a_reason() {
        let summary = run_corpus("conformance/guard");

        assert_eq!(summary.discovered, 20);
        assert_eq!(
            summary.selected, 20,
            "a recorded stdout is one observation, so a manifest test has one variant"
        );
        assert_eq!(summary.skipped, 0, "the probes are plain JavaScript");
        assert_eq!(
            summary.passed + summary.failed,
            20,
            "every probe is either measured as passing or measured as failing"
        );
        // Deliberately not asserting the pass count. It is a measurement, and pinning it here
        // would make an interpreter improvement look like a broken test.
        for case in summary
            .cases
            .iter()
            .filter(|case| !matches!(case.outcome, TestOutcome::Pass))
        {
            assert!(
                case.diagnostics
                    .first()
                    .is_some_and(|diagnostic| { diagnostic.message.contains(':') }),
                "{} failed without a phase-tagged reason",
                case.id
            );
        }
        assert!(
            summary
                .cases
                .iter()
                .all(|case| case.id == case.path && !case.id.contains('#')),
            "manifest IDs already describe their single observed invocation"
        );
    }

    /// The Effect corpus runs its bundles when the working tree has them and skips when it does
    /// not, and never fails for the difference.
    ///
    /// The bundles are not committed -- they are 2.5 MB of near-duplicate esbuild output -- so
    /// which of the two arms a checkout takes depends on whether `conformance/effect/build.sh`
    /// has been run. Asserting one arm would make this test a statement about the machine it
    /// runs on. What is actually invariant is that every case does one or the other, with a
    /// reason when it skips.
    #[test]
    fn typescript_programs_run_from_a_bundle_or_skip_with_a_reason() {
        let summary = run_corpus("conformance/effect");

        assert_eq!(summary.discovered, 12);
        assert_eq!(summary.skipped + summary.passed, 12);
        assert_eq!(
            summary.failed, 0,
            "either kiln ran the bundle and agreed, or it was never asked"
        );
        for case in &summary.cases {
            if matches!(case.outcome, TestOutcome::Pass) {
                continue;
            }
            let reason = &case.diagnostics.first().expect("a skip says why").message;
            assert!(reason.starts_with("harness: "), "{reason}");
            assert!(reason.contains("build.sh"), "{reason}");
        }
    }

    #[test]
    fn external_layout_fixture_covers_harness_variants_negative_and_unsupported() {
        let summary = run_corpus("tests/conformance/external-test262/test");

        assert_eq!(summary.discovered, 4);
        assert_eq!(
            summary.selected, 5,
            "ordinary scripts have sloppy and strict variants"
        );
        assert_eq!(
            summary.failed, 0,
            "the fixture expectations are intentional"
        );
        assert_eq!(summary.skipped, 0, "the fixture supplies its harness");
        assert_eq!(
            summary.passed, 4,
            "harness, module, and parse-negative cases execute"
        );
        assert_eq!(
            summary.unsupported, 1,
            "unsupported language features stay visible"
        );
        assert!(
            summary
                .cases
                .iter()
                .any(|case| case.id == "ordinary.js#sloppy")
        );
        assert!(
            summary
                .cases
                .iter()
                .any(|case| case.id == "ordinary.js#strict")
        );
        assert!(
            summary
                .cases
                .iter()
                .any(|case| case.id == "module.js#module")
        );
        assert!(
            summary
                .cases
                .iter()
                .any(|case| case.id == "negative-parse.js#sloppy"
                    && matches!(case.outcome, TestOutcome::Pass))
        );
        assert!(
            summary
                .cases
                .iter()
                .any(|case| case.id == "unsupported.js#sloppy"
                    && matches!(case.outcome, TestOutcome::Unsupported))
        );
    }

    #[test]
    fn one_manifest_serves_each_of_its_groups_and_the_whole_corpus() {
        let whole = run_corpus("conformance");
        let groups = [
            "conformance/guard",
            "conformance/effect",
            "conformance/regress",
            "conformance/server",
        ];
        let expected = groups
            .iter()
            .map(|group| run_corpus(group).discovered)
            .sum::<usize>();
        assert_eq!(
            whole.discovered, expected,
            "pointing at the corpus root picks up every case from each declared group"
        );
        assert!(
            whole
                .cases
                .iter()
                .any(|case| case.path == "guard/g01-spread-symbols"),
            "manifest ids do not move when a run is pointed at a different directory"
        );
    }
}
