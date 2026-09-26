//! Keeping one project's artifact cache bounded.
//!
//! `kiln-artifact-cache` decides *what* a sweep may remove and proves that removing it can only
//! cost a rebuild (see that crate's `eviction` module). This module decides *when* the daemon
//! asks for one, and answers `Request::CacheGc` on behalf of the CLI.
//!
//! # The budget
//!
//! Two gibibytes per writable layer, which is also the cache crate's own default - but not for
//! the reason written down there. That crate justifies 2 GiB as roughly twenty-five times a
//! measured 83 MB working set. That measurement has aged badly. The same `.kiln/cache`, measured
//! while this was written, held **1.5 GiB in 418 records**: 1.2 GiB of linked artifacts across
//! 114 records, 140 MB of runtime libraries, 111 MB of objects and 44 MB of generated C. It had
//! grown from 83 MB to 1.5 GiB inside a day of ordinary work, and it was gaining several hundred
//! megabytes an hour while these numbers were being taken.
//!
//! So the budget is not a comfortable ceiling above a small working set; it is a real bound on a
//! directory that would otherwise have taken the volume's remaining 23 GiB in under two days.
//! Two gibibytes is what that argument settles on:
//!
//! * It is under 9% of the free space on the volume it was measured on, so the cache cannot
//!   become the thing that fills a disk that is already 98% full.
//! * It is an order of magnitude above the 140 MB runtime-library tier, and comfortably above
//!   one full working set of the expensive tiers. A budget near or below that number would be a
//!   standing request to evict the artifact every application shares, which the cache's cost
//!   ordering exists to avoid.
//! * At the observed churn it keeps several hours of linked artifacts - the tier that costs a
//!   compile plus a link to reproduce - and gives up only the coldest of them.
//!
//! The two neighbouring choices both lose. 512 MiB is below one day's working set, so it would
//! evict artifacts minutes old and turn eviction into a standing tax. 8 GiB is a third of the
//! free space on this volume, which is not a bound at all on the machine that needs one most.
//!
//! # When a sweep runs
//!
//! Never on the critical path, which the eviction module is explicit about: not on a miss, and
//! not between a lookup and the compile it gates. A sweep here happens only after a build has
//! completely finished, is paced so that at most one runs per [`SWEEP_THRESHOLD_PERCENT`] of the
//! budget published, and runs on a detached thread so it is not even on the path of the response
//! that reports the build. A failed sweep is logged and otherwise ignored: the artifacts are on
//! disk and usable either way.
//!
//! # Native identity policy
//!
//! `EvictionPolicy::live_identities` drives the tier that collects records no key this toolchain
//! can construct again. `kiln-native` is the sole authority for that set: after a native builder
//! completes, its full stage/profile identity catalogue is remembered here and supplied to later
//! explicit or opportunistic sweeps. The daemon never reconstructs compiler, backend, ABI, target,
//! or profile tuples itself.
//!
//! Before this process has used a native builder the remembered set is empty, which safely disables
//! identity-stale eviction. That keeps interpreter-only `cache gc` independent of a C-toolchain
//! probe. Budget eviction still works, and the first native build enables the stronger tier for the
//! rest of the resident process.
//!
//! The daemon depends directly on `kiln-artifact-cache` because cache maintenance must not probe
//! the native C toolchain. A developer using interpreter-only mode should still be able to sweep
//! old native artifacts.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::thread;

use kiln_artifact_cache::{ArtifactCache, Digest, EvictionPolicy};
use kiln_proto::{CacheGcSummary, CacheLayerSweep};

/// Bytes one writable cache layer may hold. See the module documentation for the measurement.
pub(crate) const BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Share of the budget that must be published before an opportunistic sweep is worth its scan.
const SWEEP_THRESHOLD_PERCENT: u64 = 10;
/// Bytes published between opportunistic sweeps. Mirrors `EvictionPolicy::sweep_threshold_bytes`.
const SWEEP_THRESHOLD_BYTES: u64 = BUDGET_BYTES / 100 * SWEEP_THRESHOLD_PERCENT;

/// Bytes published since the last elected sweep.
///
/// Process-wide rather than per-project, like the toolchain probe: a daemon serves one project,
/// and a counter per cache root would be a map that never shrinks.
static PUBLISHED_SINCE_SWEEP: AtomicU64 = AtomicU64::new(0);
static LIVE_IDENTITIES: OnceLock<Mutex<BTreeSet<Digest>>> = OnceLock::new();

fn live_identities() -> &'static Mutex<BTreeSet<Digest>> {
    LIVE_IDENTITIES.get_or_init(|| Mutex::new(BTreeSet::new()))
}

fn remember_live_identities(identities: BTreeSet<Digest>) {
    if identities.is_empty() {
        return;
    }
    *live_identities()
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = identities;
}

fn current_live_identities() -> BTreeSet<Digest> {
    live_identities()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Where a project keeps its artifacts. The same derivation the native build path uses.
pub(crate) fn cache_root(project_root: &Path) -> PathBuf {
    project_root.join(".kiln/cache")
}

/// Sweep every writable layer of one project's cache and describe what happened.
pub(crate) fn sweep(project_root: &Path, dry_run: bool) -> Result<CacheGcSummary, String> {
    sweep_to_budget(project_root, dry_run, BUDGET_BYTES)
}

/// Sweep against an explicit per-layer budget.
///
/// The budget is the caller's policy, not the cache's: every request handler passes
/// [`BUDGET_BYTES`]. Taking it as an argument is what lets a test drive the over-budget path
/// without fabricating gigabytes of fixture.
pub(crate) fn sweep_to_budget(
    project_root: &Path,
    dry_run: bool,
    budget_bytes: u64,
) -> Result<CacheGcSummary, String> {
    sweep_to_budget_with_identities(
        project_root,
        dry_run,
        budget_bytes,
        current_live_identities(),
    )
}

fn sweep_to_budget_with_identities(
    project_root: &Path,
    dry_run: bool,
    budget_bytes: u64,
    identities: BTreeSet<Digest>,
) -> Result<CacheGcSummary, String> {
    let root = cache_root(project_root);
    let cache = ArtifactCache::new(root.clone());
    let mut policy = EvictionPolicy::with_budget(budget_bytes);
    policy.dry_run = dry_run;
    policy.live_identities = identities;
    let reports = cache.sweep(&policy).map_err(|error| error.to_string())?;

    Ok(CacheGcSummary {
        cache_root: root.to_string_lossy().into_owned(),
        dry_run,
        budget_bytes,
        layers: reports
            .iter()
            .map(|report| CacheLayerSweep {
                layer_index: report.layer_index as u64,
                bytes_before: report.bytes_before,
                bytes_after: report.bytes_after,
                records_scanned: report.records_scanned as u64,
                records_evicted: report.records_evicted as u64,
                identity_stale_evicted: report.identity_stale_evicted as u64,
                records_raced: report.records_raced as u64,
                records_within_grace: report.records_within_grace as u64,
                temporaries_skipped: report.temporaries_skipped as u64,
                orphan_markers_removed: report.orphan_markers_removed as u64,
                unrecognized_entries: report.unrecognized_entries as u64,
                over_budget_after_sweep: report.over_budget_after_sweep,
            })
            .collect(),
    })
}

/// Account for what a finished build published, and sweep if enough has accumulated.
///
/// This is the opportunistic path. It is called after a build has produced its report, so the
/// compile it would otherwise delay is already over, and the sweep itself is detached from the
/// request thread entirely.
pub(crate) fn record_publication(project_root: &Path, bytes: u64, identities: BTreeSet<Digest>) {
    remember_live_identities(identities);
    if bytes == 0 || !elect_sweeper(bytes) {
        return;
    }
    let project_root = project_root.to_path_buf();
    let identities = current_live_identities();
    // A sweep that fails, or a daemon that exits mid-sweep, costs nothing: `unlink` is atomic,
    // in-flight temporaries are never touched, and a record that survives is simply still cached.
    if let Err(error) = thread::Builder::new()
        .name("kiln-cache-sweep".to_owned())
        .spawn(move || {
            if let Err(error) =
                sweep_to_budget_with_identities(&project_root, false, BUDGET_BYTES, identities)
            {
                eprintln!("kiln-daemon: cache sweep failed: {error}");
            }
        })
    {
        eprintln!("kiln-daemon: cannot start the cache sweep thread: {error}");
    }
}

/// Add `bytes` to the pending total, reporting whether this caller should sweep.
///
/// One read-modify-write, so a burst of concurrent builds elects one sweeper rather than every
/// build that happened to observe a total above the threshold. This is what
/// `kiln_artifact_cache::SweepTrigger::record_write` does; it is reimplemented here only because
/// that type cannot be stored in a `static` without naming it.
fn elect_sweeper(bytes: u64) -> bool {
    elect(&PUBLISHED_SINCE_SWEEP, bytes, SWEEP_THRESHOLD_BYTES)
}

fn elect(counter: &AtomicU64, bytes: u64, threshold: u64) -> bool {
    let mut elected = false;
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        let next = current.saturating_add(bytes);
        elected = next >= threshold.max(1);
        Some(if elected { 0 } else { next })
    });
    elected
}

/// Bytes one finished executable build published into the cache.
///
/// Only artifacts that missed are counted: a hit republishes nothing. Per-module figures come
/// from `module_artifacts` rather than the rolled-up `sizes`, because `cache_hits.generated_c`
/// and `cache_hits.application_object` are true only when *every* module hit, so a graph build
/// where one module changed would otherwise be counted as if it had published everything.
///
/// This deliberately undercounts: the generated runtime and launcher C sources, and the shared
/// number-provider archive, are published without their sizes appearing in the report. An
/// undercount can only postpone a sweep, never provoke one.
pub(crate) fn published_bytes(report: &kiln_native::NativeBuildReport) -> u64 {
    let modules = report.module_artifacts.iter().flat_map(|module| {
        [
            (module.generated_c_cache_hit, module.generated_c_bytes),
            (module.object_cache_hit, module.object_bytes),
        ]
    });
    missed_bytes(modules.chain([
        (
            report.cache_hits.runtime_object,
            report.sizes.runtime_object,
        ),
        (
            report.cache_hits.launcher_object,
            report.sizes.launcher_object,
        ),
        (report.cache_hits.executable, report.sizes.executable),
    ]))
}

/// Bytes one finished shared-library build published into the cache.
pub(crate) fn published_shared_bytes(report: &kiln_native::NativeSharedLibraryReport) -> u64 {
    missed_bytes([
        (report.cache_hits.generated_c, report.sizes.generated_c),
        (report.cache_hits.object, report.sizes.object),
        (report.cache_hits.library, report.sizes.library),
    ])
}

/// Sum the sizes of the stages that missed. A stage that hit republished nothing.
fn missed_bytes(stages: impl IntoIterator<Item = (bool, u64)>) -> u64 {
    stages
        .into_iter()
        .filter(|(hit, _)| !hit)
        .fold(0u64, |total, (_, size)| total.saturating_add(size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_artifact_cache::{ArtifactKey, ArtifactKind, Backend, BuildProfile};
    use std::fs::{self, FileTimes, OpenOptions};
    use std::sync::atomic::AtomicU64;
    use std::time::{Duration, SystemTime};

    /// A cache record is addressed by a 64-character digest under `<kind>/<first two>/`. A sweep
    /// reads names and metadata only, never a record's bytes, so a fixture record does not have
    /// to be a real one - which is what keeps these tests off the C toolchain.
    fn seed(root: &Path, kind: &str, name: &str, size: usize, age: Duration) -> PathBuf {
        let digest: String = name.chars().cycle().take(64).collect();
        let shard = root.join(kind).join(&digest[..2]);
        fs::create_dir_all(&shard).expect("create shard");
        let path = shard.join(&digest);
        fs::write(&path, vec![b'x'; size]).expect("write record");
        let file = OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open record");
        file.set_times(FileTimes::new().set_modified(SystemTime::now() - age))
            .expect("age record");
        path
    }

    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kiln-cache-gc-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(cache_root(&dir)).expect("create cache root");
        dir
    }

    fn key(compiler: &str) -> ArtifactKey {
        ArtifactKey::new(
            ArtifactKind::GeneratedC,
            compiler,
            Backend::C,
            "host-2;c-1",
            "test-target",
            "generic",
            BuildProfile::Debug,
            "none",
            Digest::hash(b"options"),
            [Digest::hash(b"input")],
        )
    }

    fn age_record_and_marker(path: &Path) {
        let old = SystemTime::now() - Duration::from_secs(7200);
        let mut marker = path.as_os_str().to_os_string();
        marker.push(".id");
        for path in [path.to_path_buf(), PathBuf::from(marker)] {
            let file = OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("open cache record or identity marker");
            file.set_times(FileTimes::new().set_modified(old))
                .expect("age cache record or identity marker");
        }
    }

    #[test]
    fn a_dry_sweep_reports_what_it_would_remove_and_removes_nothing() {
        let project = scratch("dry");
        let root = cache_root(&project);
        let old = seed(&root, "generated-c", "a", 4096, Duration::from_secs(7200));
        let fresh = seed(&root, "generated-c", "b", 4096, Duration::ZERO);

        let summary = sweep_to_budget(&project, true, 1024).expect("dry sweep succeeds without C");
        let layer = &summary.layers[0];
        assert!(summary.dry_run);
        assert_eq!(summary.budget_bytes, 1024);
        assert_eq!(layer.records_scanned, 2);
        // The fresh record is inside the grace window, so only the old one is reachable.
        assert_eq!(layer.records_within_grace, 1);
        assert_eq!(layer.records_evicted, 1);
        assert_eq!(layer.bytes_before, 8192);
        assert!(layer.over_budget_after_sweep, "{layer:?}");
        assert!(old.exists(), "a dry run must not delete anything");
        assert!(fresh.exists());

        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn an_over_budget_sweep_gives_up_the_cheapest_rebuild_and_keeps_the_shared_runtime() {
        let project = scratch("budget");
        let root = cache_root(&project);
        let cold = Duration::from_secs(7200);
        let generated = seed(&root, "generated-c", "a", 4096, cold);
        let object = seed(&root, "native-object", "b", 4096, cold);
        let runtime = seed(&root, "runtime-library", "c", 4096, cold);

        // Room for one record. Rebuild cost decides which one survives: re-emitting C is free,
        // an object is one compile, and the runtime library is shared by every application.
        let summary = sweep_to_budget(&project, false, 4096).expect("sweep succeeds without C");
        let layer = &summary.layers[0];
        assert_eq!(layer.records_evicted, 2);
        assert_eq!(
            layer.identity_stale_evicted, 0,
            "no identity is declared live"
        );
        assert!(!layer.over_budget_after_sweep);
        assert!(
            !generated.exists(),
            "generated C is the cheapest to rebuild"
        );
        assert!(!object.exists());
        assert!(
            runtime.exists(),
            "the shared runtime is the last thing to go"
        );

        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn authoritative_identity_set_evicts_only_an_unreachable_toolchain_family() {
        let project = scratch("identity");
        let cache = ArtifactCache::new(cache_root(&project));
        let live = key("current compiler");
        let stale = key("superseded compiler");
        let live_record = cache.put(&live, b"live").expect("store live record").path;
        let stale_record = cache
            .put(&stale, b"stale")
            .expect("store stale record")
            .path;
        age_record_and_marker(&live_record);
        age_record_and_marker(&stale_record);

        let summary = sweep_to_budget_with_identities(
            &project,
            false,
            u64::MAX,
            BTreeSet::from([live.identity_digest()]),
        )
        .expect("identity-aware sweep succeeds");
        let layer = &summary.layers[0];
        assert_eq!(layer.records_evicted, 1);
        assert_eq!(layer.identity_stale_evicted, 1);
        assert!(
            live_record.exists(),
            "the current toolchain family stays live"
        );
        assert!(
            !stale_record.exists(),
            "a superseded compiler family is unreachable regardless of budget"
        );

        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn a_sweep_of_a_project_that_has_never_built_reports_an_empty_cache() {
        let project = scratch("empty");
        let summary = sweep(&project, true).expect("sweep succeeds without C");
        assert_eq!(summary.budget_bytes, BUDGET_BYTES);
        assert_eq!(summary.layers.len(), 1, "one writable layer");
        assert_eq!(summary.layers[0].records_scanned, 0);
        assert_eq!(summary.layers[0].bytes_before, 0);

        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn one_burst_of_publications_elects_exactly_one_sweeper() {
        let counter = AtomicU64::new(0);
        assert!(!elect(&counter, 40, 100));
        assert!(!elect(&counter, 40, 100));
        // The third publication crosses the threshold, and crossing it resets the counter so the
        // next publication starts a fresh window rather than electing a sweeper every time.
        assert!(elect(&counter, 40, 100));
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        assert!(!elect(&counter, 40, 100));
    }

    #[test]
    fn concurrent_publications_elect_one_sweeper_between_them() {
        let counter = AtomicU64::new(0);
        let elections: usize = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| usize::from(elect(&counter, 16, 128))))
                .collect();
            workers.into_iter().map(|w| w.join().expect("worker")).sum()
        });
        // Eight publications of an eighth of the threshold each: one crossing, not eight.
        assert_eq!(elections, 1);
    }

    #[test]
    fn only_the_stages_that_missed_count_as_published() {
        assert_eq!(missed_bytes([(true, 1000), (false, 7), (true, 2)]), 7);
        assert_eq!(missed_bytes([(true, 1), (true, 2)]), 0);
        // A saturating sum: a corrupt report can never panic the build path it is attached to.
        assert_eq!(missed_bytes([(false, u64::MAX), (false, 1)]), u64::MAX);
    }
}
