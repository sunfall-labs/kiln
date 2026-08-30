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

/// Version of the serialized cache record and key canonicalization.
pub const CACHE_FORMAT_VERSION: u16 = 1;

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
    /// Validation artifacts with assertions or sanitizers enabled.
    Check,
    /// A caller-defined profile name.
    Custom(String),
}

impl BuildProfile {
    /// Stable profile name used in key canonicalization.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
            Self::Check => "check",
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
        }
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
            let path = path_for_root(layer.root(), key);
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => return Err(io_error("read cache artifact", &path, source)),
            };
            let (payload_digest, payload) = decode_record(&path, key_digest, &bytes)?;
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
        let parent = path
            .parent()
            .expect("cache artifact path always has a parent");
        fs::create_dir_all(parent)
            .map_err(|source| io_error("create cache directory", parent, source))?;

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
        result.map(|()| StoreResult {
            digest: payload_digest,
            key_digest,
            layer_index,
            path,
            bytes_written: payload.len() as u64,
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
            let existing = match fs::read(destination) {
                Ok(existing) => existing,
                Err(_) => {
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

fn decode_record(
    path: &Path,
    expected_key: Digest,
    bytes: &[u8],
) -> Result<(Digest, Vec<u8>), CacheError> {
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
        let stored = cache.put(&first, b"generated C").expect("store succeeds");
        assert_eq!(stored.bytes_written, 11);
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
}
