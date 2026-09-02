//! Bounded growth for the layered artifact cache.
//!
//! # Why deleting a cache record is always safe
//!
//! Every record is addressed by [`ArtifactKey::digest`], which ADR 0009 requires to include the
//! artifact kind, compiler/backend identity, HostOps/runtime ABI, target, CPU policy, build
//! profile, compatibility mode, options digest, and the ordered input digests.  A key is therefore
//! a complete statement of an artifact's meaning, and the stored record additionally verifies both
//! the key digest and the payload digest before a lookup is allowed to succeed.
//!
//! The consequence is the invariant this module exists to rely on: **deleting a record can only
//! cost a rebuild, never change an answer.**  A build that would have hit the deleted record now
//! misses, regenerates the identical bytes, and republishes them under the identical key.  There is
//! no path by which eviction can substitute one artifact for another, because no other artifact can
//! ever occupy that address.  Eviction is a performance policy, not a correctness boundary, and it
//! is allowed to exist for exactly that reason.
//!
//! # Concurrency
//!
//! This module deliberately introduces **no lock**.  ADR 0009's "process mutex plus an advisory
//! lock" governs the *output publication directory* in the native coordinator, not the cache; the
//! cache's own concurrency design is lock-free and rests on same-directory temporaries plus
//! no-replace hard-link installation.  Adding a lockfile here would introduce the one failure mode
//! that design avoids: a crashed holder that blocks every later build.
//!
//! Instead the sweep is written so that concurrency needs no exclusion:
//!
//! * `unlink` is atomic, and on POSIX an already-open record survives its own deletion, so a reader
//!   mid-read is unaffected.
//! * [`ArtifactCache::lookup`] materializes the whole payload in one read before it returns, so no
//!   caller holds a cache path it intends to read later.
//! * Two sweepers racing on one record converge on "the record is gone".  `ENOENT` is a normal
//!   outcome here, not an error.  Note that `unlink` is *not* an election primitive: measured on
//!   Darwin/APFS, six threads unlinking the same sixty paths report roughly a hundred and seventy
//!   successes rather than sixty, so more than one caller can be told it won.  Nothing here depends
//!   on that being exclusive — deletion is idempotent and the outcome is identical — but it does
//!   mean [`SweepReport::records_evicted`] measures this sweeper's effort, not a partition of the
//!   records, and that no future code may treat a successful unlink as a claim of ownership.
//! * In-flight temporaries are never deleted.  A temporary is the one file whose removal *would*
//!   break a live writer, because [`ArtifactCache::put`] hard-links from it.
//! * Shard and kind directories are never removed.  Removing one could land between a concurrent
//!   writer's directory check and its hard-link, turning a good publish into a spurious failure.
//!   Empty directories are cheap; that race is not worth winning.
//! * The grace window keeps very recent records out of reach entirely, so a build that is still
//!   assembling its inputs is not made to redo work.  This is a performance guard layered on top of
//!   the safety argument above, not a substitute for it.

use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::fs::{self, FileTimes, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::{
    ArtifactCache, ArtifactKey, ArtifactKind, CacheError, CacheLayer, DIGEST_BYTES, Digest,
    StoreResult, io_error,
};

/// Version of the identity marker written beside a record.
const IDENTITY_MARKER_VERSION: u16 = 1;
const IDENTITY_MAGIC: &[u8; 8] = b"KILNIDN\0";
const IDENTITY_MARKER_BYTES: usize = 8 + 2 + DIGEST_BYTES;
/// Filename extension of the identity marker.  Records are bare 64-character digests, so an
/// extension cannot collide with one.
const IDENTITY_EXTENSION: &str = "id";

/// Default per-layer byte budget.
///
/// Chosen against a measured working cache of 83 MB in 42 records: 15 generated C at 3.9 MB, 14
/// objects at 9.1 MB, 8 linked artifacts at 36 MB, and 5 runtime libraries at 33 MB, with the
/// largest single record 8.3 MB.  Two gibibytes is roughly twenty-five times a day's working set,
/// which keeps the cache useful across many branches while making it impossible for it to become
/// the largest thing on the disk unnoticed.  Note the shape of that distribution: the two cheapest
/// tiers hold only 13 MB of the 83, so a budget set below the size of the runtime-library tier is
/// effectively a request to evict the artifact every application shares.  Configure accordingly.
pub const DEFAULT_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Default window during which a record is untouchable because a build may still be assembling it.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(600);
/// Default share of the budget that must be published before an opportunistic sweep is worthwhile.
pub const DEFAULT_SWEEP_THRESHOLD_PERCENT: u64 = 10;

/// What it costs to reproduce an evicted artifact, cheapest first.
///
/// This is the ordering that decides what the cache gives up under pressure.  It is derived from
/// the pipeline ADR 0009 describes: generated C is a pure re-emit, an object is one compile, a
/// linked artifact is a compile plus a link, and the runtime library is shared by every application
/// built on the machine, so losing it charges every one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RebuildCost {
    /// Deterministic re-emission from bytecode.
    Reemit,
    /// One invocation of the platform C compiler.
    Compile,
    /// A compile plus a link.
    CompileAndLink,
    /// Shared by every application; the last thing worth losing.
    SharedFoundation,
}

impl ArtifactKind {
    /// Every kind this cache knows how to enumerate.
    ///
    /// A sweep walks only these directory names.  An unrecognized directory under a cache root is
    /// left completely alone, so cache maintenance can never widen into a recursive delete of
    /// something this crate does not own.
    pub const ALL: [Self; 4] = [
        Self::GeneratedC,
        Self::NativeObject,
        Self::LinkedArtifact,
        Self::RuntimeLibrary,
    ];

    /// Parse the stable namespace produced by [`ArtifactKind::as_str`].
    pub fn from_namespace(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    /// What reproducing this kind of artifact costs.
    pub const fn rebuild_cost(&self) -> RebuildCost {
        match self {
            Self::GeneratedC => RebuildCost::Reemit,
            Self::NativeObject => RebuildCost::Compile,
            Self::LinkedArtifact => RebuildCost::CompileAndLink,
            Self::RuntimeLibrary => RebuildCost::SharedFoundation,
        }
    }
}

impl ArtifactKey {
    /// Digest of the toolchain identity shared by every artifact this process can still produce.
    ///
    /// This covers exactly the key fields that a toolchain or ABI bump changes for *all* artifacts
    /// at once: compiler version, backend, runtime ABI, target, CPU policy, build profile, and
    /// compatibility mode.  It deliberately excludes the artifact kind and the per-artifact
    /// `options_hash`/`input_hashes`, so one identity digest describes all four kinds built by one
    /// toolchain configuration.
    ///
    /// After a toolchain bump, records carrying a superseded identity can never be addressed again
    /// by any key this process can construct.  They are not merely cold, they are unreachable, and
    /// [`EvictionPolicy::live_identities`] is how a caller declares which identities are not.
    pub fn identity_digest(&self) -> Digest {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(b"kiln-artifact-identity\0");
        encoded.extend_from_slice(&IDENTITY_MARKER_VERSION.to_le_bytes());
        for field in [
            self.compiler_version.as_str(),
            self.backend.as_str(),
            self.runtime_abi.as_str(),
            self.target.as_str(),
            self.cpu_features.as_str(),
            self.profile.as_str(),
            self.node_compatibility.as_str(),
        ] {
            encoded.extend_from_slice(&(field.len() as u64).to_le_bytes());
            encoded.extend_from_slice(field.as_bytes());
        }
        self.realm_capabilities.encode(&mut encoded);
        Digest::hash(&encoded)
    }
}

/// How much cache a layer may keep, and what it gives up first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvictionPolicy {
    /// Maximum bytes one layer may occupy, counting records and their identity markers.
    ///
    /// The bound is bytes rather than entries on purpose.  Record sizes span three orders of
    /// magnitude — generated C in the tens of kilobytes against linked artifacts in the megabytes —
    /// so an entry count bounds nothing that matters.
    pub budget_bytes: u64,
    /// Records modified inside this window are never evicted, regardless of tier.
    pub grace: Duration,
    /// Percentage of the budget that must be published before an opportunistic sweep triggers.
    pub sweep_threshold_percent: u64,
    /// Identity digests (see [`ArtifactKey::identity_digest`]) this caller can still build under.
    ///
    /// A record whose marker names an identity outside this set is unreachable and is evicted on
    /// any sweep, budget or not.  **An empty set disables the tier entirely**, which is the safe
    /// default: nothing is called stale unless a caller positively declares what is live.
    ///
    /// List every identity your process may still build under.  Omitting one costs rebuilds; it
    /// can never cost correctness, because a key is a complete statement of an artifact's meaning.
    pub live_identities: BTreeSet<Digest>,
    /// Compute and report the sweep without deleting anything.
    pub dry_run: bool,
}

impl Default for EvictionPolicy {
    fn default() -> Self {
        Self {
            budget_bytes: DEFAULT_BUDGET_BYTES,
            grace: DEFAULT_GRACE,
            sweep_threshold_percent: DEFAULT_SWEEP_THRESHOLD_PERCENT,
            live_identities: BTreeSet::new(),
            dry_run: false,
        }
    }
}

impl EvictionPolicy {
    /// A policy with an explicit budget and otherwise-default behavior.
    pub fn with_budget(budget_bytes: u64) -> Self {
        Self {
            budget_bytes,
            ..Self::default()
        }
    }

    /// Declare an identity this caller can still build under.
    pub fn allow_identity(mut self, identity: Digest) -> Self {
        self.live_identities.insert(identity);
        self
    }

    /// Declare the identity of a representative key this caller can still build under.
    pub fn allow_identity_of(self, key: &ArtifactKey) -> Self {
        self.allow_identity(key.identity_digest())
    }

    /// Bytes that must be published before an opportunistic sweep is worthwhile.
    pub const fn sweep_threshold_bytes(&self) -> u64 {
        self.budget_bytes / 100 * self.sweep_threshold_percent
    }
}

/// What one sweep of one layer observed and did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub layer_index: usize,
    /// Bytes held by the layer before the sweep.
    pub bytes_before: u64,
    /// Bytes held by the layer after the sweep, as this sweeper accounts for it.
    pub bytes_after: u64,
    pub records_scanned: usize,
    /// Records this sweeper unlinked successfully.
    ///
    /// Under concurrent sweeps this is per-sweeper effort, not a partition of the records: see the
    /// module documentation on `unlink` not being exclusive on every platform.  Summing this across
    /// concurrent sweepers can exceed the number of records that existed.
    pub records_evicted: usize,
    /// Of those, records evicted because their identity is unreachable.
    pub identity_stale_evicted: usize,
    /// Records selected for eviction that were already gone when this sweeper reached them.
    pub records_raced: usize,
    /// Records held back only by the grace window.
    pub records_within_grace: usize,
    /// In-flight `put` temporaries, always skipped.
    pub temporaries_skipped: usize,
    /// Identity markers whose record was already gone.
    pub orphan_markers_removed: usize,
    /// Entries this crate does not recognize.  Never touched.
    pub unrecognized_entries: usize,
    /// True when the layer is still over budget because every remaining record is protected.
    pub over_budget_after_sweep: bool,
}

impl SweepReport {
    /// Bytes this sweep reclaimed.
    pub const fn bytes_reclaimed(&self) -> u64 {
        self.bytes_before.saturating_sub(self.bytes_after)
    }
}

/// Counts bytes published since the last sweep so a sweep never lands on the critical path.
///
/// Callers keep one of these next to their cache and consult it after a publish.  Keeping it out of
/// [`ArtifactCache`] keeps that type a plain value, and lets a caller decide the granularity at
/// which sweeps are paced.
#[derive(Debug, Default)]
pub struct SweepTrigger {
    written_since_sweep: AtomicU64,
}

impl SweepTrigger {
    pub const fn new() -> Self {
        Self {
            written_since_sweep: AtomicU64::new(0),
        }
    }

    /// Bytes published since the last sweep.
    pub fn pending_bytes(&self) -> u64 {
        self.written_since_sweep.load(Ordering::Relaxed)
    }

    /// Record a publish and report whether a sweep is now worthwhile.
    ///
    /// Returning `true` also resets the counter, so a burst of concurrent publishes elects one
    /// sweeper rather than all of them.
    pub fn record_write(&self, bytes: u64, policy: &EvictionPolicy) -> bool {
        let threshold = policy.sweep_threshold_bytes().max(1);
        let mut elected = false;
        // One read-modify-write, so a burst of concurrent publishes elects exactly one sweeper
        // rather than every publisher that happened to observe a total above the threshold.
        let _ = self.written_since_sweep.fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |current| {
                let next = current.saturating_add(bytes);
                elected = next >= threshold;
                Some(if elected { 0 } else { next })
            },
        );
        elected
    }

    /// Forget any pending bytes.
    pub fn reset(&self) {
        self.written_since_sweep.store(0, Ordering::Relaxed);
    }
}

/// What one pass over a layer's known namespaces found.
#[derive(Debug, Default)]
struct LayerScan {
    bytes: u64,
    candidates: Vec<Candidate>,
    orphan_markers: Vec<PathBuf>,
}

/// One record considered for eviction.
#[derive(Debug)]
struct Candidate {
    path: PathBuf,
    marker: Option<PathBuf>,
    bytes: u64,
    modified: SystemTime,
    kind: ArtifactKind,
    identity_stale: bool,
}

impl ArtifactCache {
    /// Sweep every writable layer against `policy`.
    ///
    /// Read-only layers are never swept: this process does not own them, and ADR 0009 already
    /// restricts writes to explicitly writable layers.
    pub fn sweep(&self, policy: &EvictionPolicy) -> Result<Vec<SweepReport>, CacheError> {
        let mut reports = Vec::new();
        for index in 0..self.layers().len() {
            if !self.layers()[index].is_writable() {
                continue;
            }
            reports.push(self.sweep_layer(index, policy)?);
        }
        Ok(reports)
    }

    /// Sweep one explicitly selected writable layer.  This is the entry point a `kiln cache gc`
    /// command should call.
    pub fn sweep_layer(
        &self,
        layer_index: usize,
        policy: &EvictionPolicy,
    ) -> Result<SweepReport, CacheError> {
        let layer = self
            .layers()
            .get(layer_index)
            .ok_or(CacheError::InvalidLayer { index: layer_index })?;
        if !layer.is_writable() {
            return Err(CacheError::ReadOnlyLayer { index: layer_index });
        }
        sweep_root(layer, layer_index, policy)
    }

    /// Publish an artifact, then sweep only when enough has been published to be worth it.
    ///
    /// This is the opportunistic path: a sweep never runs on a cache miss, and never between a
    /// lookup and the compile it gates.  It runs after a publish has already succeeded, and only
    /// once per `sweep_threshold_percent` of the budget written.  The [`StoreResult`] is returned
    /// even when the sweep fails, because a failed sweep must never turn a good publish into an
    /// error: the artifact is on disk and usable either way.
    pub fn put_swept(
        &self,
        trigger: &SweepTrigger,
        policy: &EvictionPolicy,
        key: &ArtifactKey,
        payload: &[u8],
    ) -> Result<(StoreResult, Option<SweepReport>), CacheError> {
        let stored = self.put(key, payload)?;
        if !trigger.record_write(stored.bytes_written, policy) {
            return Ok((stored, None));
        }
        let report = self.sweep_layer(stored.layer_index, policy).ok();
        Ok((stored, report))
    }

    /// Bytes currently held by one layer, counting records and identity markers.
    ///
    /// Purely observational: it never deletes anything, including orphaned markers.
    pub fn layer_usage(&self, layer_index: usize) -> Result<u64, CacheError> {
        let layer = self
            .layers()
            .get(layer_index)
            .ok_or(CacheError::InvalidLayer { index: layer_index })?;
        let mut report = SweepReport::default();
        let scan = scan_layer(layer.root(), &BTreeSet::new(), &mut report)?;
        Ok(scan.bytes)
    }
}

fn sweep_root(
    layer: &CacheLayer,
    layer_index: usize,
    policy: &EvictionPolicy,
) -> Result<SweepReport, CacheError> {
    let mut report = SweepReport {
        layer_index,
        ..SweepReport::default()
    };
    let scan = scan_layer(layer.root(), &policy.live_identities, &mut report)?;
    let mut bytes = scan.bytes;
    let mut candidates = scan.candidates;
    report.bytes_before = bytes;

    // A marker whose record is gone is a claim about nothing. Collecting it is free and cannot
    // mislead a later sweep; the record it described has already been evicted or removed by key.
    if !policy.dry_run {
        for marker in &scan.orphan_markers {
            if remove_if_present(marker)? {
                report.orphan_markers_removed += 1;
            }
        }
    }

    let now = SystemTime::now();
    let mut protected = 0usize;
    candidates.retain(|candidate| {
        if within_grace(candidate.modified, now, policy.grace) {
            protected += 1;
            false
        } else {
            true
        }
    });
    report.records_within_grace = protected;

    // Identity-stale first regardless of age, then cheapest-to-rebuild, then least recently used.
    //
    // The stated design put LRU first with rebuild cost as a tiebreak.  Modification times are
    // effectively continuous, so a cost tiebreak on equal mtimes would never fire, and pure LRU
    // would evict the runtime library the moment it happened to be the oldest record — the exact
    // opposite of "lose it last".  Cost is therefore the major key within a tier and LRU the minor
    // one, which is what makes the shared runtime genuinely the last thing to go.  The grace window
    // is what protects work in progress; LRU only orders what is already cold.
    candidates.sort_by(|left, right| {
        Reverse(left.identity_stale)
            .cmp(&Reverse(right.identity_stale))
            .then_with(|| left.kind.rebuild_cost().cmp(&right.kind.rebuild_cost()))
            .then_with(|| left.modified.cmp(&right.modified))
            .then_with(|| left.path.cmp(&right.path))
    });

    for candidate in candidates {
        // Identity-stale records can never be addressed again, so they are collected on every
        // sweep whether or not the layer is over budget.  Live records go only under pressure.
        if !candidate.identity_stale && bytes <= policy.budget_bytes {
            break;
        }
        if policy.dry_run {
            bytes = bytes.saturating_sub(candidate.bytes);
            report.records_evicted += 1;
            if candidate.identity_stale {
                report.identity_stale_evicted += 1;
            }
            continue;
        }
        match remove_record(&candidate) {
            Ok(true) => {
                report.records_evicted += 1;
                if candidate.identity_stale {
                    report.identity_stale_evicted += 1;
                }
            }
            // Another sweeper won the race. The bytes are reclaimed either way.
            Ok(false) => report.records_raced += 1,
            Err(error) => return Err(error),
        }
        bytes = bytes.saturating_sub(candidate.bytes);
    }

    report.bytes_after = bytes;
    report.over_budget_after_sweep = bytes > policy.budget_bytes;
    Ok(report)
}

/// Remove a record and its identity marker.  Returns whether this caller did the unlink.
fn remove_record(candidate: &Candidate) -> Result<bool, CacheError> {
    // The marker goes first: a marker without a record is meaningless, while a record without a
    // marker is simply one whose identity is unknown, which ranks as live.  Losing the record and
    // keeping the marker would leave a stale claim behind.
    if let Some(marker) = &candidate.marker {
        remove_if_present(marker)?;
    }
    remove_if_present(&candidate.path)
}

fn remove_if_present(path: &Path) -> Result<bool, CacheError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io_error("remove cache artifact", path, source)),
    }
}

fn within_grace(modified: SystemTime, now: SystemTime, grace: Duration) -> bool {
    match now.duration_since(modified) {
        Ok(age) => age < grace,
        // A modification time in the future is clock skew, not staleness. Treat it as brand new.
        Err(_) => true,
    }
}

/// Walk the known kind namespaces of one layer root.
///
/// Only [`ArtifactKind::ALL`] directory names are entered, and only files whose names are a bare
/// 64-character digest or that digest plus the marker extension are considered.  Everything else is
/// counted and left in place.
fn scan_layer(
    root: &Path,
    live_identities: &BTreeSet<Digest>,
    report: &mut SweepReport,
) -> Result<LayerScan, CacheError> {
    let mut scan = LayerScan::default();
    match fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(CacheError::Corrupt {
                path: root.to_path_buf(),
                reason: "expected a regular cache root directory".to_owned(),
            });
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(scan),
        Err(source) => return Err(io_error("inspect cache root", root, source)),
    }

    for kind in ArtifactKind::ALL {
        let kind_root = root.join(kind.as_str());
        let shards = match read_dir_optional(&kind_root)? {
            Some(shards) => shards,
            None => continue,
        };
        for shard in shards {
            let entries = match read_dir_optional(&shard)? {
                Some(entries) => entries,
                None => continue,
            };
            scan_shard(&entries, kind.clone(), live_identities, report, &mut scan)?;
        }
    }
    Ok(scan)
}

fn scan_shard(
    entries: &[PathBuf],
    kind: ArtifactKind,
    live_identities: &BTreeSet<Digest>,
    report: &mut SweepReport,
    scan: &mut LayerScan,
) -> Result<(), CacheError> {
    let mut records: Vec<(String, PathBuf, u64, SystemTime)> = Vec::new();
    let mut markers: Vec<(String, PathBuf, u64)> = Vec::new();
    let mut record_names: BTreeSet<String> = BTreeSet::new();

    for path in entries {
        let name = match path.file_name().and_then(|name| name.to_str()) {
            Some(name) => name,
            None => {
                report.unrecognized_entries += 1;
                continue;
            }
        };
        // An in-flight `put` hard-links from its temporary; deleting one turns a good publish into
        // a spurious failure. Temporaries are the single thing in this tree that is never ours.
        if name.starts_with('.') || name.contains(".tmp-") {
            report.temporaries_skipped += 1;
            continue;
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(io_error("inspect cache artifact", path, source)),
        };
        if !metadata.file_type().is_file() {
            report.unrecognized_entries += 1;
            continue;
        }
        match name.split_once('.') {
            Some((stem, IDENTITY_EXTENSION)) if is_record_name(stem) => {
                markers.push((stem.to_owned(), path.clone(), metadata.len()));
                continue;
            }
            Some(_) => {
                report.unrecognized_entries += 1;
                continue;
            }
            None => {}
        }
        if !is_record_name(name) {
            report.unrecognized_entries += 1;
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        record_names.insert(name.to_owned());
        records.push((name.to_owned(), path.clone(), metadata.len(), modified));
    }

    for (name, path, bytes, modified) in records {
        report.records_scanned += 1;
        let marker = markers
            .iter()
            .find(|(stem, _, _)| *stem == name)
            .map(|(_, marker_path, marker_bytes)| (marker_path.clone(), *marker_bytes));
        let (marker_path, marker_bytes) = match marker {
            Some((marker_path, marker_bytes)) => (Some(marker_path), marker_bytes),
            None => (None, 0),
        };
        let identity = marker_path.as_deref().and_then(read_identity_marker);
        // No marker means no claim, and no claim ranks as live. Staleness must be positively
        // established; the tier can only ever remove work it can prove is unreachable.
        let identity_stale = !live_identities.is_empty()
            && identity.is_some_and(|identity| !live_identities.contains(&identity));
        scan.bytes += bytes + marker_bytes;
        scan.candidates.push(Candidate {
            path,
            marker: marker_path,
            bytes: bytes + marker_bytes,
            modified,
            kind: kind.clone(),
            identity_stale,
        });
    }

    for (stem, path, _) in markers {
        if !record_names.contains(&stem) {
            scan.orphan_markers.push(path);
        }
    }
    Ok(())
}

fn read_dir_optional(path: &Path) -> Result<Option<Vec<PathBuf>>, CacheError> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) if source.kind() == io::ErrorKind::NotADirectory => return Ok(None),
        Err(source) => return Err(io_error("read cache directory", path, source)),
    };
    let mut paths = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => paths.push(entry.path()),
            // A concurrent sweeper or publisher can retire an entry mid-listing.
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(io_error("read cache directory", path, source)),
        }
    }
    Ok(Some(paths))
}

fn is_record_name(name: &str) -> bool {
    name.len() == DIGEST_BYTES * 2 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Path of the identity marker beside a record.
pub(crate) fn identity_marker_path(record: &Path) -> PathBuf {
    let mut name = record.as_os_str().to_os_string();
    name.push(".");
    name.push(IDENTITY_EXTENSION);
    PathBuf::from(name)
}

fn read_identity_marker(path: &Path) -> Option<Digest> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() != IDENTITY_MARKER_BYTES || &bytes[..IDENTITY_MAGIC.len()] != IDENTITY_MAGIC {
        return None;
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    if version != IDENTITY_MARKER_VERSION {
        return None;
    }
    let mut digest = [0u8; DIGEST_BYTES];
    digest.copy_from_slice(&bytes[10..]);
    Some(Digest::from_bytes(digest))
}

/// Write the identity marker beside a freshly installed record.
///
/// Best effort by construction.  The record is the artifact; the marker is only a hint that lets a
/// later sweep prove unreachability.  A missing or unreadable marker degrades eviction to plain
/// LRU, so no failure here is worth failing a publish over — least of all on a full disk, which is
/// exactly when the cache most needs to be swept.
pub(crate) fn write_identity_marker(record: &Path, identity: Digest, temp_suffix: &str) {
    let marker = identity_marker_path(record);
    if fs::symlink_metadata(&marker).is_ok() {
        return;
    }
    let mut contents = Vec::with_capacity(IDENTITY_MARKER_BYTES);
    contents.extend_from_slice(IDENTITY_MAGIC);
    contents.extend_from_slice(&IDENTITY_MARKER_VERSION.to_le_bytes());
    contents.extend_from_slice(identity.as_bytes());

    let parent = match marker.parent() {
        Some(parent) => parent,
        None => return,
    };
    let temp = parent.join(format!(".identity.tmp-{temp_suffix}"));
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(&contents)?;
            file.flush()
        });
    if written.is_err() {
        let _ = fs::remove_file(&temp);
        return;
    }
    // Same no-replace installation the record itself uses.
    let _ = fs::hard_link(&temp, &marker);
    let _ = fs::remove_file(&temp);
}

/// Make a record's modification time its last-use time.
///
/// Best effort, and only on layers this process may write.  atime is not usable for this: it is
/// widely mounted `relatime` or `noatime`, so it is not a last-use clock at all.  One `futimens` on
/// a path that has already read and digested the whole record is not a measurable cost.
pub(crate) fn refresh_recency(path: &Path) {
    let now = SystemTime::now();
    // `write(true)` opens without truncating; the record's bytes are untouched and its digests
    // remain valid. Only the inode's timestamp changes.
    if let Ok(file) = OpenOptions::new().write(true).open(path) {
        let _ = file.set_times(FileTimes::new().set_modified(now));
    }
    let marker = identity_marker_path(path);
    if let Ok(file) = OpenOptions::new().write(true).open(marker) {
        let _ = file.set_times(FileTimes::new().set_modified(now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Backend, BuildProfile, Lookup};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::thread;

    fn key(kind: ArtifactKind, compiler: &str, seed: &str) -> ArtifactKey {
        ArtifactKey::new(
            kind,
            compiler,
            Backend::C,
            "host-1",
            "aarch64-apple-darwin",
            "neon",
            BuildProfile::Debug,
            "node-22",
            Digest::hash(b"options"),
            [Digest::hash(seed.as_bytes())],
        )
    }

    fn scratch(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "kiln-artifact-evict-{name}-{}-{stamp}",
            std::process::id()
        ))
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    /// Backdate a record so a test can express recency without waiting.
    fn age(path: &Path, ago: Duration) {
        let when = SystemTime::now() - ago;
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open record to age it");
        file.set_times(FileTimes::new().set_modified(when))
            .expect("set record modification time");
    }

    fn modified_of(path: &Path) -> SystemTime {
        fs::metadata(path)
            .expect("record exists")
            .modified()
            .expect("mtime")
    }

    fn immediate(budget: u64) -> EvictionPolicy {
        EvictionPolicy {
            grace: Duration::ZERO,
            ..EvictionPolicy::with_budget(budget)
        }
    }

    fn exists(cache: &ArtifactCache, key: &ArtifactKey) -> bool {
        matches!(cache.lookup(key).expect("lookup"), Lookup::Hit(_))
    }

    #[test]
    fn identity_digest_ignores_per_artifact_inputs_but_tracks_the_toolchain() {
        let generated = key(ArtifactKind::GeneratedC, "compiler-1", "a");
        let mut other_inputs = generated.clone();
        other_inputs.input_hashes = vec![Digest::hash(b"different")];
        other_inputs.options_hash = Digest::hash(b"different");
        other_inputs.kind = ArtifactKind::LinkedArtifact;
        assert_eq!(
            generated.identity_digest(),
            other_inputs.identity_digest(),
            "one toolchain identity must cover every kind and every input"
        );

        for mutate in [
            (|k: &mut ArtifactKey| k.compiler_version = "compiler-2".into())
                as fn(&mut ArtifactKey),
            |k: &mut ArtifactKey| k.runtime_abi = "host-2".into(),
            |k: &mut ArtifactKey| k.target = "x86_64-unknown-linux-gnu".into(),
            |k: &mut ArtifactKey| k.cpu_features = "baseline".into(),
            |k: &mut ArtifactKey| k.profile = BuildProfile::Release,
            |k: &mut ArtifactKey| k.node_compatibility = "node-24".into(),
            |k: &mut ArtifactKey| {
                k.realm_capabilities.clock = crate::CapabilityIdentity::Host;
            },
            |k: &mut ArtifactKey| k.backend = Backend::Custom("llvm".into()),
        ] {
            let mut bumped = generated.clone();
            mutate(&mut bumped);
            assert_ne!(generated.identity_digest(), bumped.identity_digest());
        }
    }

    #[test]
    fn a_hit_refreshes_recency_and_can_be_turned_off() {
        let root = scratch("recency");
        let cache = ArtifactCache::new(&root);
        let entry = key(ArtifactKind::GeneratedC, "compiler-1", "a");
        let stored = cache.put(&entry, b"payload").expect("put");
        age(&stored.path, Duration::from_secs(4 * 3600));
        let stale = modified_of(&stored.path);

        assert!(matches!(
            cache.lookup(&entry).expect("lookup"),
            Lookup::Hit(_)
        ));
        let refreshed = modified_of(&stored.path);
        assert!(
            refreshed > stale,
            "a hit must stamp the record as recently used"
        );
        // The record itself is untouched: the payload still validates.
        match cache.lookup(&entry).expect("lookup") {
            Lookup::Hit(artifact) => assert_eq!(artifact.bytes, b"payload"),
            Lookup::Miss => panic!("record must survive a recency refresh"),
        }

        let quiet = ArtifactCache::new(&root).with_recency_refresh(false);
        age(&stored.path, Duration::from_secs(4 * 3600));
        let before = modified_of(&stored.path);
        assert!(matches!(
            quiet.lookup(&entry).expect("lookup"),
            Lookup::Hit(_)
        ));
        assert_eq!(before, modified_of(&stored.path));
        cleanup(&root);
    }

    #[test]
    fn a_read_only_layer_is_never_touched_by_recency_or_sweeping() {
        let shared = scratch("readonly-shared");
        let local = scratch("readonly-local");
        let entry = key(ArtifactKind::GeneratedC, "compiler-1", "a");
        let seeding = ArtifactCache::new(&shared);
        let stored = seeding.put(&entry, b"shared").expect("put");
        age(&stored.path, Duration::from_secs(4 * 3600));
        let before = modified_of(&stored.path);

        let cache = ArtifactCache::with_layers([
            CacheLayer::read_only(&shared),
            CacheLayer::writable(&local),
        ]);
        assert!(matches!(
            cache.lookup(&entry).expect("lookup"),
            Lookup::Hit(_)
        ));
        assert_eq!(
            before,
            modified_of(&stored.path),
            "a lookup must not write into a layer this process does not own"
        );

        assert!(matches!(
            cache.sweep_layer(0, &immediate(0)),
            Err(CacheError::ReadOnlyLayer { index: 0 })
        ));
        let reports = cache.sweep(&immediate(0)).expect("sweep");
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].layer_index, 1);
        assert!(exists(&cache, &entry), "the shared layer must be intact");
        cleanup(&shared);
        cleanup(&local);
    }

    #[test]
    fn a_byte_budget_is_respected_and_an_evicted_record_is_only_a_rebuild() {
        let root = scratch("budget");
        let cache = ArtifactCache::new(&root);
        let mut stored = Vec::new();
        for index in 0..8 {
            let entry = key(
                ArtifactKind::GeneratedC,
                "compiler-1",
                &format!("seed-{index}"),
            );
            let payload = vec![b'x'; 64];
            let result = cache.put(&entry, &payload).expect("put");
            age(&result.path, Duration::from_secs(3600 + index as u64 * 60));
            stored.push((entry, payload, result.path));
        }
        let usage = cache.layer_usage(0).expect("usage");
        assert!(usage > 0);

        let budget = usage / 2;
        let report = cache.sweep_layer(0, &immediate(budget)).expect("sweep");
        assert_eq!(report.records_scanned, 8);
        assert!(report.records_evicted > 0);
        assert!(!report.over_budget_after_sweep);
        let after = cache.layer_usage(0).expect("usage");
        assert!(
            after <= budget,
            "layer holds {after} bytes against a {budget} byte budget"
        );
        assert_eq!(report.bytes_after, after);
        assert!(report.bytes_reclaimed() > 0);

        // Eviction costs a rebuild and nothing else: republishing the same key restores the same
        // artifact under the same address.
        for (entry, payload, _) in &stored {
            if !exists(&cache, entry) {
                cache
                    .put(entry, payload)
                    .expect("republish evicted artifact");
            }
            match cache.lookup(entry).expect("lookup") {
                Lookup::Hit(artifact) => assert_eq!(&artifact.bytes, payload),
                Lookup::Miss => panic!("republished artifact must be readable"),
            }
        }
        cleanup(&root);
    }

    #[test]
    fn a_sweep_under_budget_evicts_nothing() {
        let root = scratch("under-budget");
        let cache = ArtifactCache::new(&root);
        let entry = key(ArtifactKind::GeneratedC, "compiler-1", "a");
        cache.put(&entry, b"payload").expect("put");
        let report = cache.sweep_layer(0, &immediate(u64::MAX)).expect("sweep");
        assert_eq!(report.records_evicted, 0);
        assert!(exists(&cache, &entry));
        cleanup(&root);
    }

    #[test]
    fn identity_stale_records_go_first_and_go_without_budget_pressure() {
        let root = scratch("identity");
        let cache = ArtifactCache::new(&root);
        // The runtime library is the most expensive thing to lose, and it is the newest record.
        // Being unreachable outranks both.
        let stale = key(ArtifactKind::RuntimeLibrary, "compiler-1", "runtime");
        let live = key(ArtifactKind::GeneratedC, "compiler-2", "module");
        let stale_stored = cache.put(&stale, b"old-runtime").expect("put");
        let live_stored = cache.put(&live, b"module-c").expect("put");
        age(&stale_stored.path, Duration::from_secs(60));
        age(&live_stored.path, Duration::from_secs(4 * 3600));

        let policy = EvictionPolicy {
            grace: Duration::ZERO,
            ..EvictionPolicy::with_budget(u64::MAX)
        }
        .allow_identity_of(&live);

        let report = cache.sweep_layer(0, &policy).expect("sweep");
        assert_eq!(report.identity_stale_evicted, 1);
        assert_eq!(report.records_evicted, 1);
        assert!(
            !exists(&cache, &stale),
            "a record no reachable key can address is free to delete"
        );
        assert!(
            exists(&cache, &live),
            "the live record must survive even though it is older and cheaper"
        );
        assert!(
            !identity_marker_path(&stale_stored.path).exists(),
            "the identity marker must go with its record"
        );
        cleanup(&root);
    }

    #[test]
    fn an_empty_live_identity_set_never_calls_anything_stale() {
        let root = scratch("identity-off");
        let cache = ArtifactCache::new(&root);
        let entry = key(ArtifactKind::RuntimeLibrary, "compiler-1", "runtime");
        cache.put(&entry, b"runtime").expect("put");
        let report = cache
            .sweep_layer(
                0,
                &EvictionPolicy {
                    grace: Duration::ZERO,
                    ..EvictionPolicy::with_budget(u64::MAX)
                },
            )
            .expect("sweep");
        assert_eq!(report.identity_stale_evicted, 0);
        assert!(exists(&cache, &entry));
        cleanup(&root);
    }

    #[test]
    fn a_record_without_an_identity_marker_ranks_as_live() {
        let root = scratch("identity-unknown");
        let cache = ArtifactCache::new(&root);
        let unmarked = key(ArtifactKind::RuntimeLibrary, "compiler-1", "runtime");
        let live = key(ArtifactKind::GeneratedC, "compiler-2", "module");
        let unmarked_stored = cache.put(&unmarked, b"runtime").expect("put");
        cache.put(&live, b"module").expect("put");
        // Records written before identity markers existed have none.
        fs::remove_file(identity_marker_path(&unmarked_stored.path)).expect("drop marker");
        age(&unmarked_stored.path, Duration::from_secs(4 * 3600));

        let policy = EvictionPolicy {
            grace: Duration::ZERO,
            ..EvictionPolicy::with_budget(u64::MAX)
        }
        .allow_identity_of(&live);
        let report = cache.sweep_layer(0, &policy).expect("sweep");
        assert_eq!(report.identity_stale_evicted, 0);
        assert!(
            exists(&cache, &unmarked),
            "staleness must be positively established, never assumed"
        );
        cleanup(&root);
    }

    #[test]
    fn rebuild_cost_orders_eviction_and_lru_orders_within_a_cost_class() {
        let root = scratch("order");
        let cache = ArtifactCache::new(&root);
        let payload = vec![b'x'; 64];

        // Two cheap re-emits and one expensive shared runtime. The runtime is by far the oldest.
        let old_c = key(ArtifactKind::GeneratedC, "compiler-1", "old");
        let new_c = key(ArtifactKind::GeneratedC, "compiler-1", "new");
        let runtime = key(ArtifactKind::RuntimeLibrary, "compiler-1", "runtime");
        let old_c_path = cache.put(&old_c, &payload).expect("put").path;
        let new_c_path = cache.put(&new_c, &payload).expect("put").path;
        let runtime_path = cache.put(&runtime, &payload).expect("put").path;
        age(&runtime_path, Duration::from_secs(48 * 3600));
        age(&old_c_path, Duration::from_secs(4 * 3600));
        age(&new_c_path, Duration::from_secs(3600));

        // Room for exactly two of the three records.
        let unit = cache.layer_usage(0).expect("usage") / 3;
        let report = cache.sweep_layer(0, &immediate(unit * 2)).expect("sweep");
        assert_eq!(report.records_evicted, 1);
        assert!(
            !exists(&cache, &old_c),
            "the cheapest artifact to rebuild goes first, and within that class the oldest"
        );
        assert!(exists(&cache, &new_c));
        assert!(
            exists(&cache, &runtime),
            "the shared runtime is the last thing to lose, despite being the oldest record"
        );

        // Tighten to one record: the second re-emit goes before the runtime.
        let report = cache.sweep_layer(0, &immediate(unit)).expect("sweep");
        assert_eq!(report.records_evicted, 1);
        assert!(!exists(&cache, &new_c));
        assert!(exists(&cache, &runtime));

        // Only under real pressure does the runtime go.
        let report = cache.sweep_layer(0, &immediate(0)).expect("sweep");
        assert_eq!(report.records_evicted, 1);
        assert!(!exists(&cache, &runtime));
        cleanup(&root);
    }

    #[test]
    fn the_grace_window_protects_a_build_that_is_still_assembling_its_inputs() {
        let root = scratch("grace");
        let cache = ArtifactCache::new(&root);
        let fresh = key(ArtifactKind::GeneratedC, "compiler-1", "fresh");
        let cold = key(ArtifactKind::GeneratedC, "compiler-1", "cold");
        let payload = vec![b'x'; 64];
        cache.put(&fresh, &payload).expect("put");
        let cold_path = cache.put(&cold, &payload).expect("put").path;
        age(&cold_path, Duration::from_secs(4 * 3600));

        // Budget zero: everything is over budget, and only the grace window holds anything back.
        let policy = EvictionPolicy {
            grace: Duration::from_secs(600),
            ..EvictionPolicy::with_budget(0)
        };
        let report = cache.sweep_layer(0, &policy).expect("sweep");
        assert_eq!(report.records_within_grace, 1);
        assert_eq!(report.records_evicted, 1);
        assert!(
            exists(&cache, &fresh),
            "a record written seconds ago is in use"
        );
        assert!(!exists(&cache, &cold));
        assert!(
            report.over_budget_after_sweep,
            "a layer still over budget because everything is protected must say so"
        );
        cleanup(&root);
    }

    #[test]
    fn the_grace_window_also_protects_a_freshly_written_stale_identity() {
        let root = scratch("grace-identity");
        let cache = ArtifactCache::new(&root);
        let stale = key(ArtifactKind::GeneratedC, "compiler-1", "a");
        let live = key(ArtifactKind::GeneratedC, "compiler-2", "b");
        cache.put(&stale, b"payload").expect("put");
        let policy = EvictionPolicy {
            grace: Duration::from_secs(600),
            ..EvictionPolicy::with_budget(u64::MAX)
        }
        .allow_identity_of(&live);
        let report = cache.sweep_layer(0, &policy).expect("sweep");
        assert_eq!(report.identity_stale_evicted, 0);
        assert_eq!(report.records_within_grace, 1);
        assert!(
            exists(&cache, &stale),
            "another process may still be mid-build against the identity we just retired"
        );
        cleanup(&root);
    }

    #[test]
    fn a_clock_skewed_future_record_is_treated_as_brand_new() {
        let root = scratch("skew");
        let cache = ArtifactCache::new(&root);
        let entry = key(ArtifactKind::GeneratedC, "compiler-1", "a");
        let stored = cache.put(&entry, b"payload").expect("put");
        let future = SystemTime::now() + Duration::from_secs(3600);
        OpenOptions::new()
            .write(true)
            .open(&stored.path)
            .expect("open")
            .set_times(FileTimes::new().set_modified(future))
            .expect("set future mtime");
        let report = cache.sweep_layer(0, &immediate(0)).expect("sweep");
        assert_eq!(report.records_evicted, 0);
        assert_eq!(report.records_within_grace, 1);
        assert!(exists(&cache, &entry));
        cleanup(&root);
    }

    #[test]
    fn in_flight_temporaries_and_foreign_entries_are_never_deleted() {
        let root = scratch("temporaries");
        let cache = ArtifactCache::new(&root);
        let entry = key(ArtifactKind::GeneratedC, "compiler-1", "a");
        let stored = cache.put(&entry, b"payload").expect("put");
        let shard = stored.path.parent().expect("shard").to_path_buf();

        // Exactly the shape `put` creates while a record is mid-flight. Deleting one breaks the
        // hard link a live writer is about to install.
        let temporary = shard.join(format!(
            ".{}.tmp-9999-0-0",
            stored.path.file_name().expect("name").to_string_lossy()
        ));
        fs::write(&temporary, b"in flight").expect("write temporary");
        let foreign = shard.join("README");
        fs::write(&foreign, b"not ours").expect("write foreign entry");
        let unknown_kind = root.join("some-future-kind");
        fs::create_dir_all(&unknown_kind).expect("create dir");
        fs::write(unknown_kind.join("keep-me"), b"future").expect("write");

        let report = cache.sweep_layer(0, &immediate(0)).expect("sweep");
        assert_eq!(report.temporaries_skipped, 1);
        assert_eq!(report.unrecognized_entries, 1);
        assert_eq!(report.records_evicted, 1);
        assert!(
            temporary.exists(),
            "an in-flight temporary is never ours to delete"
        );
        assert!(foreign.exists());
        assert!(
            unknown_kind.join("keep-me").exists(),
            "a sweep must never widen into a recursive delete of a namespace it does not own"
        );
        assert!(!stored.path.exists());
        cleanup(&root);
    }

    #[test]
    fn an_orphan_identity_marker_is_collected() {
        let root = scratch("orphan");
        let cache = ArtifactCache::new(&root);
        let entry = key(ArtifactKind::GeneratedC, "compiler-1", "a");
        let stored = cache.put(&entry, b"payload").expect("put");
        let marker = identity_marker_path(&stored.path);
        assert!(marker.exists());
        fs::remove_file(&stored.path).expect("remove record");

        let report = cache.sweep_layer(0, &immediate(u64::MAX)).expect("sweep");
        assert_eq!(report.orphan_markers_removed, 1);
        assert_eq!(report.records_scanned, 0);
        assert!(!marker.exists());
        cleanup(&root);
    }

    #[test]
    fn removing_a_key_removes_its_identity_marker() {
        let root = scratch("remove-marker");
        let cache = ArtifactCache::new(&root);
        let entry = key(ArtifactKind::GeneratedC, "compiler-1", "a");
        let stored = cache.put(&entry, b"payload").expect("put");
        assert!(cache.remove(&entry).expect("remove"));
        assert!(!identity_marker_path(&stored.path).exists());
        assert_eq!(cache.layer_usage(0).expect("usage"), 0);
        cleanup(&root);
    }

    #[test]
    fn layer_usage_is_purely_observational() {
        let root = scratch("usage-pure");
        let cache = ArtifactCache::new(&root);
        let entry = key(ArtifactKind::GeneratedC, "compiler-1", "a");
        let stored = cache.put(&entry, b"payload").expect("put");
        let marker = identity_marker_path(&stored.path);
        fs::remove_file(&stored.path).expect("orphan the marker");
        assert!(cache.layer_usage(0).expect("usage") == 0);
        assert!(
            marker.exists(),
            "a usage query must not delete anything, not even an orphaned marker"
        );
        cleanup(&root);
    }

    #[test]
    fn a_dry_run_reports_without_deleting() {
        let root = scratch("dry-run");
        let cache = ArtifactCache::new(&root);
        let entry = key(ArtifactKind::GeneratedC, "compiler-1", "a");
        let stored = cache.put(&entry, b"payload").expect("put");
        age(&stored.path, Duration::from_secs(4 * 3600));
        let policy = EvictionPolicy {
            grace: Duration::ZERO,
            dry_run: true,
            ..EvictionPolicy::with_budget(0)
        };
        let report = cache.sweep_layer(0, &policy).expect("sweep");
        assert_eq!(report.records_evicted, 1);
        assert!(report.bytes_reclaimed() > 0);
        assert!(exists(&cache, &entry), "a dry run must not delete anything");

        // Not even an orphaned marker.
        let orphan = key(ArtifactKind::GeneratedC, "compiler-1", "orphan");
        let orphan_stored = cache.put(&orphan, b"payload").expect("put");
        let orphan_marker = identity_marker_path(&orphan_stored.path);
        fs::remove_file(&orphan_stored.path).expect("orphan the marker");
        let report = cache.sweep_layer(0, &policy).expect("sweep");
        assert_eq!(report.orphan_markers_removed, 0);
        assert!(orphan_marker.exists());
        cleanup(&root);
    }

    #[test]
    fn the_trigger_elects_one_sweeper_per_threshold_of_bytes() {
        let policy = EvictionPolicy::with_budget(1_000);
        assert_eq!(policy.sweep_threshold_bytes(), 100);
        let trigger = SweepTrigger::new();
        assert!(!trigger.record_write(60, &policy));
        assert_eq!(trigger.pending_bytes(), 60);
        assert!(trigger.record_write(60, &policy));
        assert_eq!(trigger.pending_bytes(), 0);
        assert!(!trigger.record_write(10, &policy));
        trigger.reset();
        assert_eq!(trigger.pending_bytes(), 0);

        // Concurrent publishers must elect one sweeper per threshold crossed, not one each.
        let shared = Arc::new(SweepTrigger::new());
        let policy = Arc::new(EvictionPolicy::with_budget(1_000));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let trigger = Arc::clone(&shared);
                let policy = Arc::clone(&policy);
                thread::spawn(move || {
                    (0..100)
                        .filter(|_| trigger.record_write(10, &policy))
                        .count()
                })
            })
            .collect();
        let elections: usize = handles.into_iter().map(|h| h.join().expect("join")).sum();
        // 8 threads * 100 writes * 10 bytes = 8000 bytes against a 100 byte threshold.
        assert_eq!(elections, 80);
    }

    #[test]
    fn put_swept_returns_the_artifact_and_sweeps_only_when_worthwhile() {
        let root = scratch("put-swept");
        let cache = ArtifactCache::new(&root);
        let trigger = SweepTrigger::new();
        let policy = EvictionPolicy {
            grace: Duration::ZERO,
            ..EvictionPolicy::with_budget(1_000)
        };
        let mut swept = 0;
        for index in 0..40 {
            let entry = key(ArtifactKind::GeneratedC, "compiler-1", &format!("s{index}"));
            let (stored, report) = cache
                .put_swept(&trigger, &policy, &entry, &[b'x'; 64])
                .expect("put");
            assert_eq!(stored.bytes_written, 64);
            if report.is_some() {
                swept += 1;
            }
        }
        assert!(swept > 0, "publishing 2560 bytes must trigger sweeps");
        assert!(
            swept < 40,
            "a sweep must not run on every publish; it is paced by bytes written"
        );
        assert!(cache.layer_usage(0).expect("usage") <= policy.budget_bytes);
        cleanup(&root);
    }

    #[test]
    fn concurrent_sweeps_and_publishes_never_fail_and_never_break_a_writer() {
        let root = scratch("concurrent");
        let cache = Arc::new(ArtifactCache::new(&root));
        // Seed enough records that the sweepers have real work while the publisher runs.
        for index in 0..40 {
            let entry = key(
                ArtifactKind::GeneratedC,
                "compiler-1",
                &format!("seed-{index}"),
            );
            cache.put(&entry, &[b'x'; 64]).expect("seed");
        }

        let stop = Arc::new(AtomicBool::new(false));
        // Budget zero and no grace: the sweepers try to delete everything, continuously, while a
        // publisher installs records through the same shards. The only guarantee relied on is that
        // unlink is atomic, that ENOENT is a normal outcome, and that temporaries and directories
        // are never removed. No lock is taken by anyone.
        let sweepers: Vec<_> = (0..4)
            .map(|_| {
                let cache = Arc::clone(&cache);
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    let policy = EvictionPolicy {
                        grace: Duration::ZERO,
                        ..EvictionPolicy::with_budget(0)
                    };
                    let mut sweeps = 0usize;
                    while !stop.load(Ordering::Relaxed) {
                        cache
                            .sweep_layer(0, &policy)
                            .expect("a sweep must not fail");
                        sweeps += 1;
                    }
                    sweeps
                })
            })
            .collect();

        let publishers: Vec<_> = (0..4)
            .map(|worker| {
                let cache = Arc::clone(&cache);
                thread::spawn(move || {
                    for index in 0..150 {
                        let entry = key(
                            ArtifactKind::NativeObject,
                            "compiler-1",
                            &format!("w{worker}-{index}"),
                        );
                        let payload = format!("payload-{worker}-{index}").into_bytes();
                        cache
                            .put(&entry, &payload)
                            .expect("a publish must not fail while sweepers run");
                        // Whatever a lookup returns, it can never be another artifact's bytes.
                        if let Lookup::Hit(artifact) = cache.lookup(&entry).expect("lookup") {
                            assert_eq!(artifact.bytes, payload);
                        }
                    }
                })
            })
            .collect();

        for publisher in publishers {
            publisher.join().expect("publisher joined");
        }
        stop.store(true, Ordering::Relaxed);
        let total: usize = sweepers
            .into_iter()
            .map(|sweeper| sweeper.join().expect("sweeper joined"))
            .sum();
        assert!(total > 0, "sweepers must have run");

        // A final sweep with no publisher racing drains the layer completely.
        let report = cache.sweep_layer(0, &immediate(0)).expect("final sweep");
        assert!(!report.over_budget_after_sweep);
        assert_eq!(cache.layer_usage(0).expect("usage"), 0);
        cleanup(&root);
    }

    /// Spawned by [`concurrent_sweeps_across_processes_converge`]; not a test on its own.
    #[test]
    #[ignore = "re-executed as a child process by the cross-process sweep test"]
    fn child_sweeper() {
        let root = match std::env::var("KILN_ARTIFACT_CACHE_SWEEP_ROOT") {
            Ok(root) => PathBuf::from(root),
            Err(_) => return,
        };
        let cache = ArtifactCache::new(&root);
        for _ in 0..12 {
            cache
                .sweep_layer(0, &immediate(0))
                .expect("a sweep must not fail while another process publishes");
        }
        fs::write(root.join(format!("swept-{}", std::process::id())), b"done")
            .expect("record that this child ran");
    }

    /// The case the daemon actually faces: separate processes, no shared memory, no lock.
    #[test]
    fn concurrent_sweeps_across_processes_converge() {
        let root = scratch("cross-process");
        let cache = ArtifactCache::new(&root);
        for index in 0..40 {
            let entry = key(ArtifactKind::GeneratedC, "compiler-1", &format!("x{index}"));
            let stored = cache.put(&entry, &[b'x'; 64]).expect("seed");
            age(&stored.path, Duration::from_secs(3600));
        }

        let exe = std::env::current_exe().expect("test binary path");
        let children: Vec<_> = (0..3)
            .map(|_| {
                std::process::Command::new(&exe)
                    .args([
                        "--exact",
                        "--ignored",
                        "--test-threads=1",
                        "eviction::tests::child_sweeper",
                    ])
                    .env("KILN_ARTIFACT_CACHE_SWEEP_ROOT", &root)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .expect("spawn a sweeping child process")
            })
            .collect();

        // Publish through the same shards the other processes are sweeping. A publish that lost its
        // temporary, or found its shard directory removed, would fail here.
        let mut published = Vec::new();
        for index in 0..300 {
            let entry = key(
                ArtifactKind::NativeObject,
                "compiler-1",
                &format!("p{index}"),
            );
            let payload = format!("payload-{index}").into_bytes();
            cache
                .put(&entry, &payload)
                .expect("a publish must not fail while other processes sweep");
            // A record either is not there or is exactly the artifact its key names.
            if let Lookup::Hit(artifact) = cache.lookup(&entry).expect("lookup") {
                assert_eq!(artifact.bytes, payload);
            }
            published.push((entry, payload));
        }

        for mut child in children {
            assert!(
                child.wait().expect("wait for child").success(),
                "a child sweeper must exit cleanly"
            );
        }
        let ran = fs::read_dir(&root)
            .expect("read root")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("swept-"))
            .count();
        assert_eq!(ran, 3, "all three child processes must actually have swept");

        // Whatever survived is still exactly what its key names, and anything evicted is one
        // republish away from being back.
        for (entry, payload) in &published {
            if !exists(&cache, entry) {
                cache.put(entry, payload).expect("republish");
            }
            match cache.lookup(entry).expect("lookup") {
                Lookup::Hit(artifact) => assert_eq!(&artifact.bytes, payload),
                Lookup::Miss => panic!("a republished artifact must be readable"),
            }
        }
        cleanup(&root);
    }

    #[test]
    fn two_sweepers_racing_one_record_report_the_race_instead_of_failing() {
        let root = scratch("race");
        let cache = Arc::new(ArtifactCache::new(&root));
        for index in 0..60 {
            let entry = key(ArtifactKind::GeneratedC, "compiler-1", &format!("r{index}"));
            let stored = cache.put(&entry, &[b'x'; 64]).expect("put");
            age(&stored.path, Duration::from_secs(3600));
        }
        let handles: Vec<_> = (0..6)
            .map(|_| {
                let cache = Arc::clone(&cache);
                thread::spawn(move || {
                    cache
                        .sweep_layer(0, &immediate(0))
                        .expect("a racing sweep must not fail")
                })
            })
            .collect();
        let reports: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("join"))
            .collect();

        // Every sweeper saw all sixty records and resolved each one to either "I removed it" or
        // "it was already gone". Neither outcome is an error, and no sweeper is starved.
        for report in &reports {
            assert_eq!(report.records_scanned, 60);
            assert_eq!(report.records_evicted + report.records_raced, 60);
            assert!(!report.over_budget_after_sweep);
        }
        // Deliberately not asserting that the successes sum to *exactly* sixty. `unlink` is not an
        // election primitive: on Darwin/APFS several threads are told they won the same path, so
        // the successes there run to about a hundred and seventy. The guarantee this design rests
        // on is convergence, asserted next, plus the fact that a content-addressed record can only
        // ever be rebuilt, never replaced.
        //
        // Nor that every sweeper evicted something. That is a fairness property and nothing here
        // provides one: this module's header says `records_evicted` measures a sweeper's own
        // effort rather than a partition of the records, so a sweeper that arrives after another
        // has finished evicts nothing and races sixty times -- the race resolving exactly as
        // designed, not a starved thread. It only ever held because Darwin's non-exclusive
        // `unlink` hands every thread plenty of wins; where `unlink` really is an election, as on
        // Linux, a loaded machine finishes one sweeper before the last has started and the
        // assertion fails on a cache that behaved perfectly.
        //
        // What must hold is coverage: every record was removed by somebody.
        let evicted: usize = reports.iter().map(|report| report.records_evicted).sum();
        assert!(
            evicted >= 60,
            "sixty records must each have been evicted by some sweeper, saw {evicted}"
        );
        assert_eq!(cache.layer_usage(0).expect("usage"), 0);
        for index in 0..60 {
            let entry = key(ArtifactKind::GeneratedC, "compiler-1", &format!("r{index}"));
            assert!(matches!(
                cache.lookup(&entry).expect("lookup"),
                Lookup::Miss
            ));
        }
        cleanup(&root);
    }
}
