//! A small, deterministic, layered cache for compiler artifacts.
//!
//! Cache entries are addressed by an [`ArtifactKey`], never by a source path.  The key includes
//! the compiler/backend/runtime contracts and all ordered input hashes, while the on-disk record
//! contains both the key digest and a payload digest.  Consequently a missing entry is distinct
//! from a malformed or stale entry: callers can report corruption instead of silently compiling
//! with a different artifact.
//!
//! Writes use a uniquely named temporary file in the destination directory, flush it to disk, and
//! atomically install it without replacing an existing content address.  The API intentionally
//! offers removal for one exact key only; cache maintenance must not accidentally turn into a broad
//! recursive deletion.

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub use kiln_digest::{DIGEST_BYTES, Digest, DigestParseError};

mod eviction;

pub use eviction::{
    DEFAULT_BUDGET_BYTES, DEFAULT_GRACE, DEFAULT_SWEEP_THRESHOLD_PERCENT, EvictionPolicy,
    RebuildCost, SweepReport, SweepTrigger,
};

/// Version of the serialized cache record and key canonicalization.
pub const CACHE_FORMAT_VERSION: u16 = 3;

const MAGIC: &[u8; 8] = b"KILNART\0";
const HEADER_BYTES: usize = 8 + 2 + DIGEST_BYTES + DIGEST_BYTES + 8;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The kind of artifact represented by a cache entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArtifactKind {
    /// Deterministic C emitted for one or more compiled modules.
    GeneratedC,
    /// A native object produced from generated C.
    NativeObject,
    /// A linked executable or library.
    LinkedArtifact,
    /// A runtime library, useful when the runtime is built separately from an application.
    RuntimeLibrary,
}

impl ArtifactKind {
    /// Stable namespace used in paths and key canonicalization.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::GeneratedC => "generated-c",
            Self::NativeObject => "native-object",
            Self::LinkedArtifact => "linked-artifact",
            Self::RuntimeLibrary => "runtime-library",
        }
    }
}

/// The native or interpreted backend producing an artifact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Backend {
    /// The interpreter's serialized/compiled representation.
    Interpreter,
    /// The C emitter/compiler pipeline.
    C,
    /// A forward-compatible backend name supplied by a caller.
    Custom(String),
}

/// Canonical environmental policy for one realm artifact.
///
/// This type belongs beside the key rather than in a caller-built string so every producer uses
/// one unambiguous encoding. Values returned at runtime by `Host` variants are not artifact inputs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RealmCapabilityIdentity {
    pub clock: CapabilityIdentity,
    pub entropy: CapabilityIdentity,
    pub timezone: CapabilityIdentity,
    pub locale: CapabilityIdentity,
}

impl Default for RealmCapabilityIdentity {
    fn default() -> Self {
        Self {
            clock: CapabilityIdentity::Virtual,
            entropy: CapabilityIdentity::Fixed,
            timezone: CapabilityIdentity::Utc,
            locale: CapabilityIdentity::Default,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CapabilityIdentity {
    Virtual,
    Fixed,
    Utc,
    Default,
    Host,
    Seeded(u64),
    Named(String),
}

impl RealmCapabilityIdentity {
    fn encode(&self, output: &mut Vec<u8>) {
        for capability in [&self.clock, &self.entropy, &self.timezone, &self.locale] {
            capability.encode(output);
        }
    }
}

impl CapabilityIdentity {
    fn encode(&self, output: &mut Vec<u8>) {
        match self {
            Self::Virtual => put_text(output, "virtual"),
            Self::Fixed => put_text(output, "fixed"),
            Self::Utc => put_text(output, "utc"),
            Self::Default => put_text(output, "default"),
            Self::Host => put_text(output, "host"),
            Self::Seeded(seed) => {
                put_text(output, "seeded");
                output.extend_from_slice(&seed.to_le_bytes());
            }
            Self::Named(value) => {
                put_text(output, "named");
                put_text(output, value);
            }
        }
    }
}

impl Backend {
    /// Stable backend name used in key canonicalization.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Interpreter => "interpreter",
            Self::C => "c",
            Self::Custom(name) => name,
        }
    }
}

/// Optimization/build profile included in every native artifact key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BuildProfile {
    /// Fast, minimally optimized development artifacts.
    Debug,
    /// Optimized artifacts intended for release.
    Release,
    /// Size-optimized release artifacts.
    ReleaseSize,
    /// Validation artifacts with assertions or sanitizers enabled.
    Check,
    /// Release artifacts carrying profiler instrumentation. Its own bucket so an instrumented
    /// object can never satisfy a release build, or the other way round.
    Profile,
    /// A caller-defined profile name.
    Custom(String),
}

impl BuildProfile {
    /// Stable profile name used in key canonicalization.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
            Self::ReleaseSize => "release-size",
            Self::Check => "check",
            Self::Profile => "profile",
            Self::Custom(name) => name,
        }
    }
}

/// All inputs that affect one artifact's meaning.
///
/// `input_hashes` is intentionally ordered.  For a linked artifact, changing object order can
/// change symbol resolution or deterministic output, so treating the inputs as a set would make
/// an unsound cache key.  Callers should sort only when their pipeline itself is order-independent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactKey {
    pub kind: ArtifactKind,
    pub compiler_version: String,
    pub backend: Backend,
    pub runtime_abi: String,
    pub target: String,
    pub cpu_features: String,
    pub profile: BuildProfile,
    pub node_compatibility: String,
    /// Realm environmental policy. Host values are runtime inputs and never appear here.
    pub realm_capabilities: RealmCapabilityIdentity,
    pub options_hash: Digest,
    pub input_hashes: Vec<Digest>,
}

impl ArtifactKey {
    /// Construct a key with the common fields.  Optional target/profile details can be filled in
    /// directly on the returned value before it is passed to the cache.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: ArtifactKind,
        compiler_version: impl Into<String>,
        backend: Backend,
        runtime_abi: impl Into<String>,
        target: impl Into<String>,
        cpu_features: impl Into<String>,
        profile: BuildProfile,
        node_compatibility: impl Into<String>,
        options_hash: Digest,
        input_hashes: impl IntoIterator<Item = Digest>,
    ) -> Self {
        Self {
            kind,
            compiler_version: compiler_version.into(),
            backend,
            runtime_abi: runtime_abi.into(),
            target: target.into(),
            cpu_features: cpu_features.into(),
            profile,
            node_compatibility: node_compatibility.into(),
            realm_capabilities: RealmCapabilityIdentity::default(),
            options_hash,
            input_hashes: input_hashes.into_iter().collect(),
        }
    }

    /// Return the content address for this key.
    pub fn digest(&self) -> Digest {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(b"kiln-artifact-key\0");
        encoded.extend_from_slice(&CACHE_FORMAT_VERSION.to_le_bytes());
        put_text(&mut encoded, self.kind.as_str());
        put_text(&mut encoded, &self.compiler_version);
        put_text(&mut encoded, self.backend.as_str());
        put_text(&mut encoded, &self.runtime_abi);
        put_text(&mut encoded, &self.target);
        put_text(&mut encoded, &self.cpu_features);
        put_text(&mut encoded, self.profile.as_str());
        put_text(&mut encoded, &self.node_compatibility);
        self.realm_capabilities.encode(&mut encoded);
        put_digest(&mut encoded, self.options_hash);
        encoded.extend_from_slice(&(self.input_hashes.len() as u64).to_le_bytes());
        for input in &self.input_hashes {
            put_digest(&mut encoded, *input);
        }
        Digest::hash(&encoded)
    }
}

fn put_text(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(&(value.len() as u64).to_le_bytes());
    output.extend_from_slice(value.as_bytes());
}

fn put_digest(output: &mut Vec<u8>, value: Digest) {
    output.extend_from_slice(value.as_bytes());
}

/// One physical cache layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheLayer {
    root: PathBuf,
    writable: bool,
}

impl CacheLayer {
    /// Create a writable layer.  Its directory is created lazily on the first write.
    pub fn writable(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            writable: true,
        }
    }

    /// Create a read-only layer, useful for a shared or prepopulated cache.
    pub fn read_only(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            writable: false,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub const fn is_writable(&self) -> bool {
        self.writable
    }
}

/// A cache lookup result.  `Miss` means no layer has an entry; corruption is returned as an error.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    Hit(Artifact),
    Miss,
}

/// A validated cache payload and its provenance.
#[derive(Debug, PartialEq, Eq)]
pub struct Artifact {
    pub bytes: Vec<u8>,
    pub digest: Digest,
    pub key_digest: Digest,
    pub layer_index: usize,
    pub path: PathBuf,
}

/// The result of an atomic store.
#[derive(Debug, PartialEq, Eq)]
pub struct StoreResult {
    pub digest: Digest,
    pub key_digest: Digest,
    pub layer_index: usize,
    pub path: PathBuf,
    pub bytes_written: u64,
}

/// Errors from reading or writing cache entries.
#[derive(Debug)]
pub enum CacheError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Corrupt {
        path: PathBuf,
        reason: String,
    },
    NoWritableLayer,
    ReadOnlyLayer {
        index: usize,
    },
    InvalidLayer {
        index: usize,
    },
}

impl Display for CacheError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "could not {operation} {}: {source}",
                path.display()
            ),
            Self::Corrupt { path, reason } => {
                write!(
                    formatter,
                    "corrupt cache artifact {}: {reason}",
                    path.display()
                )
            }
            Self::NoWritableLayer => formatter.write_str("artifact cache has no writable layer"),
            Self::ReadOnlyLayer { index } => {
                write!(formatter, "cache layer index {index} is read-only")
            }
            Self::InvalidLayer { index } => {
                write!(formatter, "cache layer index {index} is invalid")
            }
        }
    }
}

impl Error for CacheError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Corrupt { .. }
            | Self::NoWritableLayer
            | Self::ReadOnlyLayer { .. }
            | Self::InvalidLayer { .. } => None,
        }
    }
}

/// A layered content-addressed artifact cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactCache {
    layers: Vec<CacheLayer>,
    refresh_recency_on_hit: bool,
}

impl ArtifactCache {
    /// Make a cache with one lazily-created writable layer.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_layers([CacheLayer::writable(root)])
    }

    /// Make a cache from ordered layers.  Lookups search in order; stores use the first writable
    /// layer.  A corrupt entry stops lookup with [`CacheError::Corrupt`] instead of falling through
    /// to an older artifact that could hide the problem.
    pub fn with_layers(layers: impl IntoIterator<Item = CacheLayer>) -> Self {
        Self {
            layers: layers.into_iter().collect(),
            refresh_recency_on_hit: true,
        }
    }

    /// Choose whether a hit stamps the record's modification time as its last-use time.
    ///
    /// On by default, because without it the cache has no last-use clock and eviction can only
    /// order by build time. Turn it off for a cache that must never write during a lookup.
    #[must_use]
    pub fn with_recency_refresh(mut self, refresh_recency_on_hit: bool) -> Self {
        self.refresh_recency_on_hit = refresh_recency_on_hit;
        self
    }

    /// Whether a hit refreshes the record's last-use time.
    pub const fn refreshes_recency(&self) -> bool {
        self.refresh_recency_on_hit
    }

    pub fn layers(&self) -> &[CacheLayer] {
        &self.layers
    }

    /// Return the deterministic location for a key in a particular layer.
    pub fn path_for_layer(
        &self,
        layer_index: usize,
        key: &ArtifactKey,
    ) -> Result<PathBuf, CacheError> {
        let layer = self
            .layers
            .get(layer_index)
            .ok_or(CacheError::InvalidLayer { index: layer_index })?;
        Ok(path_for_root(layer.root(), key))
    }

    /// Return the deterministic location in the first writable layer.
    pub fn path_for(&self, key: &ArtifactKey) -> Result<PathBuf, CacheError> {
        let layer_index = self
            .layers
            .iter()
            .position(CacheLayer::is_writable)
            .ok_or(CacheError::NoWritableLayer)?;
        self.path_for_layer(layer_index, key)
    }

    /// Find and validate an artifact in the ordered layers.
    pub fn lookup(&self, key: &ArtifactKey) -> Result<Lookup, CacheError> {
        let key_digest = key.digest();
        for (layer_index, layer) in self.layers.iter().enumerate() {
            if !cache_parent_exists(layer.root(), key)? {
                continue;
            }
            let path = path_for_root(layer.root(), key);
            let bytes = match read_regular_file(&path, "read cache artifact")? {
                Some(bytes) => bytes,
                None => continue,
            };
            let (payload_digest, payload) = decode_record(&path, key_digest, &bytes)?;
            // A hit is a use. Stamping the record's mtime here is what makes eviction's LRU order
            // mean "least recently used" rather than "least recently built". It is best effort and
            // metadata-only: the payload and both digests are unchanged, so a failure (a read-only
            // filesystem, a record owned by another user) can only cost eviction accuracy.
            if self.refresh_recency_on_hit && layer.is_writable() {
                eviction::refresh_recency(&path);
            }
            return Ok(Lookup::Hit(Artifact {
                bytes: payload,
                digest: payload_digest,
                key_digest,
                layer_index,
                path,
            }));
        }
        Ok(Lookup::Miss)
    }

    /// Alias for [`ArtifactCache::lookup`] for callers that prefer read terminology.
    pub fn read(&self, key: &ArtifactKey) -> Result<Lookup, CacheError> {
        self.lookup(key)
    }

    /// Check whether an artifact exists in cache and return its payload digest and size without
    /// reading or allocating the entire payload into memory.
    pub fn lookup_digest(&self, key: &ArtifactKey) -> Result<Option<(Digest, u64)>, CacheError> {
        let key_digest = key.digest();
        for layer in &self.layers {
            if !cache_parent_exists(layer.root(), key)? {
                continue;
            }
            let path = path_for_root(layer.root(), key);
            let mut file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(io_error("open cache artifact header", &path, err)),
            };
            let mut header = [0u8; HEADER_BYTES];
            use std::io::Read;
            if file.read_exact(&mut header).is_err() {
                continue;
            }
            if let Ok((payload_digest, payload_len)) = decode_header(&path, key_digest, &header) {
                if self.refresh_recency_on_hit && layer.is_writable() {
                    eviction::refresh_recency(&path);
                }
                return Ok(Some((payload_digest, payload_len as u64)));
            }
        }
        Ok(None)
    }

    /// Atomically store an artifact in the first writable layer.
    ///
    /// An existing record for the same key is accepted only when its payload is identical. A
    /// different payload is reported as corruption instead of replacing the record.
    pub fn put(&self, key: &ArtifactKey, payload: &[u8]) -> Result<StoreResult, CacheError> {
        let layer_index = self
            .layers
            .iter()
            .position(CacheLayer::is_writable)
            .ok_or(CacheError::NoWritableLayer)?;
        self.put_in_layer(layer_index, key, payload)
    }

    /// Atomically store an artifact in an explicitly selected writable layer.
    ///
    /// An existing record for the same key is accepted only when its payload is identical.
    pub fn put_in_layer(
        &self,
        layer_index: usize,
        key: &ArtifactKey,
        payload: &[u8],
    ) -> Result<StoreResult, CacheError> {
        let layer = self
            .layers
            .get(layer_index)
            .ok_or(CacheError::InvalidLayer { index: layer_index })?;
        if !layer.is_writable() {
            return Err(CacheError::ReadOnlyLayer { index: layer_index });
        }

        let key_digest = key.digest();
        let payload_digest = Digest::hash(payload);
        let path = path_for_root(layer.root(), key);
        let parent = ensure_cache_parent(layer.root(), key)?;

        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let temp_path = parent.join(format!(
            ".{}.tmp-{}-{counter}-{timestamp}",
            path.file_name()
                .expect("cache artifact path has a filename")
                .to_string_lossy(),
            std::process::id()
        ));
        let result = write_atomic(&temp_path, &path, key_digest, payload_digest, payload);
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result.map(|()| {
            // Record the toolchain identity beside the record so a later sweep can prove that a
            // record is unreachable after a toolchain or ABI bump. Best effort: the record is the
            // artifact, the marker is only an eviction hint.
            eviction::write_identity_marker(
                &path,
                key.identity_digest(),
                &format!("{}-{counter}-{timestamp}", std::process::id()),
            );
            StoreResult {
                digest: payload_digest,
                key_digest,
                layer_index,
                path,
                bytes_written: payload.len() as u64,
            }
        })
    }

    /// Remove one exact key from the first writable layer, returning whether it existed.
    ///
    /// There is deliberately no `clear` or recursive-delete operation on this type.
    pub fn remove(&self, key: &ArtifactKey) -> Result<bool, CacheError> {
        let layer_index = self
            .layers
            .iter()
            .position(CacheLayer::is_writable)
            .ok_or(CacheError::NoWritableLayer)?;
        self.remove_from_layer(layer_index, key)
    }

    /// Remove one exact key from an explicitly selected writable layer.
    pub fn remove_from_layer(
        &self,
        layer_index: usize,
        key: &ArtifactKey,
    ) -> Result<bool, CacheError> {
        let layer = self
            .layers
            .get(layer_index)
            .ok_or(CacheError::InvalidLayer { index: layer_index })?;
        if !layer.is_writable() {
            return Err(CacheError::ReadOnlyLayer { index: layer_index });
        }
        let path = path_for_root(layer.root(), key);
        if !cache_parent_exists(layer.root(), key)? {
            return Ok(false);
        }
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            }
            Ok(_) => return Err(non_regular_path(&path, "cache record")),
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(source) => return Err(io_error("inspect cache artifact", &path, source)),
        }
        let marker = eviction::identity_marker_path(&path);
        match fs::remove_file(&marker) {
            Ok(()) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(io_error("remove cache identity marker", &marker, source)),
        }
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(io_error("remove cache artifact", &path, source)),
        }
    }
}

fn path_for_root(root: &Path, key: &ArtifactKey) -> PathBuf {
    let digest = key.digest().to_hex();
    root.join(key.kind.as_str()).join(&digest[..2]).join(digest)
}

fn cache_parent_paths(root: &Path, key: &ArtifactKey) -> (PathBuf, PathBuf) {
    let digest = key.digest().to_hex();
    let kind = root.join(key.kind.as_str());
    let shard = kind.join(&digest[..2]);
    (kind, shard)
}

/// Return whether the complete cache parent exists without traversing a symlink below the
/// caller-selected root. The root itself may intentionally be a symlink to a cache location.
fn cache_parent_exists(root: &Path, key: &ArtifactKey) -> Result<bool, CacheError> {
    match fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(non_regular_path(root, "cache root directory")),
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(io_error("inspect cache root", root, source)),
    }
    let (kind, shard) = cache_parent_paths(root, key);
    for path in [&kind, &shard] {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err(non_regular_path(path, "cache shard directory")),
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(source) => return Err(io_error("inspect cache directory", path, source)),
        }
    }
    Ok(true)
}

fn ensure_cache_parent(root: &Path, key: &ArtifactKey) -> Result<PathBuf, CacheError> {
    fs::create_dir_all(root).map_err(|source| io_error("create cache root", root, source))?;
    let root_metadata =
        fs::metadata(root).map_err(|source| io_error("inspect cache root", root, source))?;
    if !root_metadata.is_dir() {
        return Err(non_regular_path(root, "cache root directory"));
    }
    let (kind, shard) = cache_parent_paths(root, key);
    ensure_real_directory(&kind)?;
    ensure_real_directory(&shard)?;
    Ok(shard)
}

fn ensure_real_directory(path: &Path) -> Result<(), CacheError> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path)
                .map_err(|source| io_error("inspect cache directory", path, source))?;
            if metadata.file_type().is_dir() {
                Ok(())
            } else {
                Err(non_regular_path(path, "cache shard directory"))
            }
        }
        Err(source) => Err(io_error("create cache directory", path, source)),
    }
}

fn read_regular_file(path: &Path, operation: &'static str) -> Result<Option<Vec<u8>>, CacheError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => return Err(non_regular_path(path, "cache record")),
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io_error("inspect cache artifact", path, source)),
    }
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        // Eviction deliberately does not lock readers. If a sweeper unlinks the record after
        // the metadata check above, lookup degrades to the same cache miss it would have seen
        // before that check rather than turning an ordinary eviction race into an I/O failure.
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error(operation, path, source)),
    }
}

fn non_regular_path(path: &Path, expected: &str) -> CacheError {
    CacheError::Corrupt {
        path: path.to_path_buf(),
        reason: format!("expected a regular {expected}"),
    }
}

fn write_atomic(
    temp_path: &Path,
    destination: &Path,
    key_digest: Digest,
    payload_digest: Digest,
    payload: &[u8],
) -> Result<(), CacheError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp_path)
        .map_err(|source| io_error("create temporary cache artifact", temp_path, source))?;
    let header = encode_header(key_digest, payload_digest, payload.len() as u64);
    if let Err(source) = file
        .write_all(&header)
        .and_then(|()| file.write_all(payload))
    {
        return Err(io_error(
            "write temporary cache artifact",
            temp_path,
            source,
        ));
    }
    file.flush()
        .map_err(|source| io_error("flush temporary cache artifact", temp_path, source))?;
    file.sync_all()
        .map_err(|source| io_error("sync temporary cache artifact", temp_path, source))?;
    drop(file);
    // Both names are in one directory, so a hard link installs the complete, synced inode
    // atomically and, unlike POSIX rename, never replaces an existing content address. Replacing
    // would hide nondeterministic artifact production: one key is a claim that exactly one payload
    // can exist.
    match fs::hard_link(temp_path, destination) {
        Ok(()) => fs::remove_file(temp_path)
            .map_err(|source| io_error("remove installed cache temporary", temp_path, source)),
        Err(install_error) if install_error.kind() == io::ErrorKind::AlreadyExists => {
            // A concurrent writer of the same key may already have won. Accept that record only
            // after fully validating that it is byte-identical to our intended payload.
            let existing = match read_regular_file(destination, "read cache artifact")? {
                Some(existing) => existing,
                None => {
                    return Err(io_error(
                        "install cache artifact",
                        destination,
                        install_error,
                    ));
                }
            };
            let (existing_digest, existing_payload) =
                decode_record(destination, key_digest, &existing)?;
            if existing_digest != payload_digest || existing_payload != payload {
                return Err(CacheError::Corrupt {
                    path: destination.to_path_buf(),
                    reason: "same cache key already contains a different payload".to_owned(),
                });
            }
            fs::remove_file(temp_path)
                .map_err(|source| io_error("remove redundant cache artifact", temp_path, source))
        }
        Err(source) => Err(io_error("install cache artifact", destination, source)),
    }
}

fn encode_header(
    key_digest: Digest,
    payload_digest: Digest,
    payload_len: u64,
) -> [u8; HEADER_BYTES] {
    let mut header = [0; HEADER_BYTES];
    header[..MAGIC.len()].copy_from_slice(MAGIC);
    let mut offset = MAGIC.len();
    header[offset..offset + 2].copy_from_slice(&CACHE_FORMAT_VERSION.to_le_bytes());
    offset += 2;
    header[offset..offset + DIGEST_BYTES].copy_from_slice(key_digest.as_bytes());
    offset += DIGEST_BYTES;
    header[offset..offset + DIGEST_BYTES].copy_from_slice(payload_digest.as_bytes());
    offset += DIGEST_BYTES;
    header[offset..].copy_from_slice(&payload_len.to_le_bytes());
    header
}

fn decode_header(
    path: &Path,
    expected_key: Digest,
    bytes: &[u8],
) -> Result<(Digest, usize), CacheError> {
    let corrupt = |reason: String| CacheError::Corrupt {
        path: path.to_owned(),
        reason,
    };
    if bytes.len() < HEADER_BYTES {
        return Err(corrupt("record is shorter than its header".to_owned()));
    }
    if &bytes[..MAGIC.len()] != MAGIC {
        return Err(corrupt("record magic does not match".to_owned()));
    }
    let version_offset = MAGIC.len();
    let version = u16::from_le_bytes([bytes[version_offset], bytes[version_offset + 1]]);
    if version != CACHE_FORMAT_VERSION {
        return Err(corrupt(format!(
            "record version {version} is not supported (expected {CACHE_FORMAT_VERSION})"
        )));
    }
    let mut offset = version_offset + 2;
    let mut key_bytes = [0; DIGEST_BYTES];
    key_bytes.copy_from_slice(&bytes[offset..offset + DIGEST_BYTES]);
    let record_key = Digest::from_bytes(key_bytes);
    if record_key != expected_key {
        return Err(corrupt(
            "record key digest does not match requested key".to_owned(),
        ));
    }
    offset += DIGEST_BYTES;
    let mut payload_bytes = [0; DIGEST_BYTES];
    payload_bytes.copy_from_slice(&bytes[offset..offset + DIGEST_BYTES]);
    let payload_digest = Digest::from_bytes(payload_bytes);
    offset += DIGEST_BYTES;
    let mut length_bytes = [0; 8];
    length_bytes.copy_from_slice(&bytes[offset..offset + 8]);
    let payload_len = u64::from_le_bytes(length_bytes);
    let payload_len_usize = usize::try_from(payload_len)
        .map_err(|_| corrupt("record payload length does not fit in this process".to_owned()))?;
    Ok((payload_digest, payload_len_usize))
}

fn decode_record(
    path: &Path,
    expected_key: Digest,
    bytes: &[u8],
) -> Result<(Digest, Vec<u8>), CacheError> {
    let corrupt = |reason: String| CacheError::Corrupt {
        path: path.to_owned(),
        reason,
    };
    let (payload_digest, payload_len_usize) = decode_header(path, expected_key, bytes)?;
    let expected_total = HEADER_BYTES
        .checked_add(payload_len_usize)
        .ok_or_else(|| corrupt("record payload length overflows record size".to_owned()))?;
    if bytes.len() != expected_total {
        return Err(corrupt(format!(
            "record length is {}, expected {expected_total}",
            bytes.len()
        )));
    }
    let payload = bytes[HEADER_BYTES..].to_vec();
    if Digest::hash(&payload) != payload_digest {
        return Err(corrupt(
            "record payload digest does not match contents".to_owned(),
        ));
    }
    Ok((payload_digest, payload))
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> CacheError {
    CacheError::Io {
        operation,
        path: path.to_owned(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn key(seed: &str) -> ArtifactKey {
        ArtifactKey::new(
            ArtifactKind::GeneratedC,
            "compiler-1",
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

    #[test]
    fn realm_capability_policy_participates_in_artifact_identity() {
        let deterministic = key("realm-capabilities");
        assert_eq!(
            deterministic.realm_capabilities,
            RealmCapabilityIdentity::default()
        );
        let mut host = deterministic.clone();
        host.realm_capabilities.clock = CapabilityIdentity::Host;
        assert_ne!(deterministic.digest(), host.digest());

        let mut named = deterministic.clone();
        named.realm_capabilities.timezone = CapabilityIdentity::Named("America/Denver".into());
        assert_ne!(deterministic.digest(), named.digest());
        assert_ne!(host.digest(), named.digest());
    }

    fn scratch(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "kiln-artifact-cache-{name}-{}-{stamp}",
            std::process::id()
        ))
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    fn temporary_file_count(parent: &Path) -> usize {
        fs::read_dir(parent)
            .expect("artifact parent exists")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count()
    }

    #[test]
    fn sha256_matches_reference_vectors() {
        assert_eq!(
            Digest::hash(b"").to_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            Digest::hash(b"abc").to_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let digest = Digest::hash(b"Kiln");
        assert_eq!(
            Digest::from_hex(&digest.to_hex()).expect("round trip"),
            digest
        );
        assert!(Digest::from_hex("nope").is_err());
    }

    #[test]
    fn key_is_path_independent_and_input_order_sensitive() {
        let first = key("one");
        let mut second = first.clone();
        second.compiler_version = "compiler-2".into();
        assert_ne!(first.digest(), second.digest());
        second = first.clone();
        second.input_hashes.push(Digest::hash(b"two"));
        assert_ne!(first.digest(), second.digest());
        let mut reversed = second.clone();
        reversed.input_hashes.reverse();
        assert_ne!(second.digest(), reversed.digest());
        assert_eq!(
            path_for_root(Path::new("/cache"), &first),
            path_for_root(Path::new("/cache"), &first)
        );
    }

    #[test]
    fn put_lookup_miss_and_remove_are_exact() {
        let root = scratch("basic");
        let cache = ArtifactCache::new(&root);
        let first = key("one");
        let second = key("two");
        assert_eq!(cache.lookup(&first).expect("lookup succeeds"), Lookup::Miss);
        assert_eq!(cache.lookup_digest(&first).expect("lookup succeeds"), None);
        let stored = cache.put(&first, b"generated C").expect("store succeeds");
        assert_eq!(stored.bytes_written, 11);
        assert_eq!(
            cache.lookup_digest(&first).expect("lookup succeeds"),
            Some((Digest::hash(b"generated C"), 11))
        );
        let Lookup::Hit(hit) = cache.lookup(&first).expect("lookup succeeds") else {
            panic!("stored artifact should hit");
        };
        assert_eq!(hit.bytes, b"generated C");
        assert_eq!(hit.digest, Digest::hash(b"generated C"));
        assert_eq!(
            cache.lookup(&second).expect("lookup succeeds"),
            Lookup::Miss
        );
        assert!(cache.remove(&first).expect("remove succeeds"));
        assert!(!cache.remove(&first).expect("second remove is a miss"));
        assert_eq!(cache.lookup(&first).expect("lookup succeeds"), Lookup::Miss);
        cleanup(&root);
    }

    #[test]
    fn concurrent_identical_puts_share_one_record_and_clean_temporary_files() {
        let root = scratch("atomic-identical");
        let cache = ArtifactCache::new(&root);
        let artifact_key = key("atomic-identical");
        let mut handles = Vec::new();
        for _ in 0..8 {
            let cache = cache.clone();
            let artifact_key = artifact_key.clone();
            handles.push(std::thread::spawn(move || {
                cache
                    .put(&artifact_key, b"identical payload")
                    .expect("atomic store succeeds");
            }));
        }
        for handle in handles {
            handle.join().expect("store thread succeeds");
        }

        let Lookup::Hit(hit) = cache.lookup(&artifact_key).expect("lookup succeeds") else {
            panic!("concurrent stores should leave a hit");
        };
        assert_eq!(hit.bytes, b"identical payload");
        let parent = hit.path.parent().expect("artifact path has parent");
        assert_eq!(
            temporary_file_count(parent),
            0,
            "atomic store should clean temporary files"
        );
        cleanup(&root);
    }

    #[test]
    fn concurrent_conflicting_puts_choose_one_payload_without_replacement() {
        let root = scratch("atomic-conflict");
        let cache = ArtifactCache::new(&root);
        let artifact_key = key("atomic-conflict");
        let mut handles = Vec::new();
        for index in 0..8 {
            let cache = cache.clone();
            let artifact_key = artifact_key.clone();
            handles.push(std::thread::spawn(move || {
                let payload = format!("payload-{index}").into_bytes();
                let result = cache.put(&artifact_key, &payload);
                (payload, result)
            }));
        }

        let mut winners = Vec::new();
        let mut conflicts = 0;
        for handle in handles {
            let (payload, result) = handle.join().expect("store thread succeeds");
            match result {
                Ok(_) => winners.push(payload),
                Err(CacheError::Corrupt { reason, .. }) if reason.contains("different payload") => {
                    conflicts += 1;
                }
                Err(error) => panic!("unexpected concurrent store error: {error}"),
            }
        }
        assert_eq!(winners.len(), 1, "exactly one payload installs the key");
        assert_eq!(conflicts, 7, "every other payload reports a conflict");

        let Lookup::Hit(hit) = cache.lookup(&artifact_key).expect("lookup succeeds") else {
            panic!("winning concurrent store should leave a hit");
        };
        assert_eq!(hit.bytes, winners.pop().expect("one winner"));
        let parent = hit.path.parent().expect("artifact path has parent");
        assert_eq!(
            temporary_file_count(parent),
            0,
            "conflicting stores should clean temporary files"
        );
        cleanup(&root);
    }

    #[test]
    fn an_existing_key_cannot_be_replaced_by_a_different_payload() {
        let root = scratch("conflicting-payload");
        let cache = ArtifactCache::new(&root);
        let artifact_key = key("conflicting-payload");
        cache
            .put(&artifact_key, b"first payload")
            .expect("first store succeeds");

        let error = cache
            .put(&artifact_key, b"different payload")
            .expect_err("one content-addressed key must have one payload");
        assert!(
            matches!(error, CacheError::Corrupt { ref reason, .. } if reason.contains("different payload")),
            "{error}"
        );
        let Lookup::Hit(hit) = cache.lookup(&artifact_key).expect("lookup succeeds") else {
            panic!("original artifact remains present");
        };
        assert_eq!(hit.bytes, b"first payload");
        cleanup(&root);
    }

    #[test]
    fn corruption_is_reported_instead_of_becoming_a_miss() {
        let root = scratch("corruption");
        let cache = ArtifactCache::new(&root);
        let original = key("one");
        let other = key("two");
        cache.put(&original, b"payload").expect("store succeeds");
        let original_path = cache.path_for(&original).expect("path succeeds");
        fs::write(&original_path, b"partial record").expect("corrupt record");
        let error = cache
            .lookup(&original)
            .expect_err("corruption must be visible");
        assert!(matches!(error, CacheError::Corrupt { .. }));

        let other_path = cache.path_for(&other).expect("path succeeds");
        fs::create_dir_all(other_path.parent().expect("path has parent")).expect("create parent");
        fs::copy(&original_path, &other_path).expect("copy corrupt record");
        let error = cache
            .lookup(&other)
            .expect_err("key mismatch must be visible");
        assert!(matches!(error, CacheError::Corrupt { .. }));

        assert!(cache.remove(&original).expect("remove corrupt record"));
        cache.put(&original, b"payload").expect("restore record");
        let path = cache.path_for(&original).expect("path succeeds");
        let mut record = fs::read(&path).expect("read record");
        *record.last_mut().expect("record has payload") ^= 1;
        fs::write(&path, record).expect("tamper payload");
        let error = cache
            .lookup(&original)
            .expect_err("payload corruption must be visible");
        assert!(matches!(error, CacheError::Corrupt { .. }));
        cleanup(&root);
    }

    #[test]
    fn a_directory_at_a_record_path_is_corruption() {
        let root = scratch("directory-record");
        let cache = ArtifactCache::new(&root);
        let artifact_key = key("directory-record");
        let path = cache.path_for(&artifact_key).expect("path succeeds");
        fs::create_dir_all(&path).expect("create directory in place of record");

        assert!(matches!(
            cache.lookup(&artifact_key),
            Err(CacheError::Corrupt { .. })
        ));
        assert!(matches!(
            cache.put(&artifact_key, b"payload"),
            Err(CacheError::Corrupt { .. })
        ));
        assert!(matches!(
            cache.remove(&artifact_key),
            Err(CacheError::Corrupt { .. })
        ));
        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_cache_root_remains_usable() {
        use std::os::unix::fs::symlink;

        let actual = scratch("root-target");
        let alias = scratch("root-alias");
        fs::create_dir_all(&actual).expect("create cache target");
        symlink(&actual, &alias).expect("create cache root symlink");
        let cache = ArtifactCache::new(&alias);
        let artifact_key = key("root-symlink");

        cache
            .put(&artifact_key, b"payload")
            .expect("store through symlinked root");
        let Lookup::Hit(hit) = cache.lookup(&artifact_key).expect("lookup through root") else {
            panic!("stored artifact should hit");
        };
        assert_eq!(hit.bytes, b"payload");

        fs::remove_file(&alias).expect("remove root symlink");
        cleanup(&actual);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_record_is_never_read_or_accepted_as_a_winner() {
        use std::os::unix::fs::symlink;

        let root = scratch("record-symlink");
        let outside = scratch("record-symlink-target");
        let cache = ArtifactCache::new(&root);
        let artifact_key = key("record-symlink");
        cache
            .put(&artifact_key, b"payload")
            .expect("seed valid record");
        let path = cache.path_for(&artifact_key).expect("path succeeds");
        fs::create_dir_all(&outside).expect("create outside directory");
        let target = outside.join("record");
        fs::rename(&path, &target).expect("move valid record outside cache");
        symlink(&target, &path).expect("replace record with symlink");

        assert!(matches!(
            cache.lookup(&artifact_key),
            Err(CacheError::Corrupt { .. })
        ));
        assert!(matches!(
            cache.put(&artifact_key, b"payload"),
            Err(CacheError::Corrupt { .. })
        ));
        assert!(cache.remove(&artifact_key).expect("remove symlink leaf"));
        assert_eq!(
            fs::read(&target).expect("outside record remains"),
            encode_record_for_test(&artifact_key, b"payload")
        );

        cleanup(&root);
        cleanup(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_cache_directories_cannot_escape_the_layer() {
        use std::os::unix::fs::symlink;

        let root = scratch("directory-symlink");
        let outside = scratch("directory-symlink-target");
        let cache = ArtifactCache::new(&root);
        let artifact_key = key("directory-symlink");
        let path = cache.path_for(&artifact_key).expect("path succeeds");
        let kind = root.join(artifact_key.kind.as_str());
        let shard = path.parent().expect("record has shard parent");
        fs::create_dir_all(&kind).expect("create kind directory");
        fs::create_dir_all(&outside).expect("create outside directory");
        symlink(&outside, shard).expect("replace shard with symlink");
        let outside_record = outside.join(path.file_name().expect("record has name"));
        fs::write(&outside_record, b"must remain").expect("seed outside file");

        assert!(matches!(
            cache.lookup(&artifact_key),
            Err(CacheError::Corrupt { .. })
        ));
        assert!(matches!(
            cache.put(&artifact_key, b"payload"),
            Err(CacheError::Corrupt { .. })
        ));
        assert!(matches!(
            cache.remove(&artifact_key),
            Err(CacheError::Corrupt { .. })
        ));
        assert_eq!(
            fs::read(&outside_record).expect("outside record remains"),
            b"must remain"
        );

        cleanup(&root);
        cleanup(&outside);
    }

    #[test]
    fn layers_read_shared_then_write_local() {
        let local = scratch("local");
        let shared = scratch("shared");
        let key = key("layered");
        let shared_cache = ArtifactCache::new(&shared);
        shared_cache
            .put(&key, b"shared")
            .expect("seed shared cache");
        let cache = ArtifactCache::with_layers([
            CacheLayer::writable(&local),
            CacheLayer::read_only(&shared),
        ]);
        let Lookup::Hit(hit) = cache.lookup(&key).expect("shared lookup succeeds") else {
            panic!("shared artifact should hit");
        };
        assert_eq!(hit.layer_index, 1);
        assert_eq!(hit.bytes, b"shared");
        cache.put(&key, b"local").expect("write local layer");
        let Lookup::Hit(hit) = cache.lookup(&key).expect("local lookup succeeds") else {
            panic!("local artifact should hit");
        };
        assert_eq!(hit.layer_index, 0);
        assert_eq!(hit.bytes, b"local");
        cleanup(&local);
        cleanup(&shared);
    }

    #[test]
    fn read_only_cache_cannot_store_or_remove() {
        let root = scratch("read-only");
        let cache = ArtifactCache::with_layers([CacheLayer::read_only(&root)]);
        let key = key("read-only");
        assert!(matches!(
            cache.put(&key, b"x"),
            Err(CacheError::NoWritableLayer)
        ));
        assert!(matches!(
            cache.remove(&key),
            Err(CacheError::NoWritableLayer)
        ));
        cleanup(&root);
    }

    fn encode_record_for_test(key: &ArtifactKey, payload: &[u8]) -> Vec<u8> {
        let mut record =
            encode_header(key.digest(), Digest::hash(payload), payload.len() as u64).to_vec();
        record.extend_from_slice(payload);
        record
    }
}
