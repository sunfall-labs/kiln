//! A small, deterministic, layered cache for compiler artifacts.
//!
//! Cache entries are addressed by an [`ArtifactKey`], never by a source path.  The key includes
//! the compiler/backend/runtime contracts and all ordered input hashes, while the on-disk record
//! contains both the key digest and a payload digest.  Consequently a missing entry is distinct
//! from a malformed or stale entry: callers can report corruption instead of silently compiling
//! with a different artifact.
//!
//! Writes use a uniquely named temporary file in the destination directory, flush it to disk, and
//! atomically rename it into place.  The API intentionally offers removal for one exact key only;
//! cache maintenance must not accidentally turn into a broad recursive deletion.

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Version of the serialized cache record and key canonicalization.
pub const CACHE_FORMAT_VERSION: u16 = 1;

const DIGEST_BYTES: usize = 32;
const MAGIC: &[u8; 8] = b"KILNART\0";
const HEADER_BYTES: usize = 8 + 2 + DIGEST_BYTES + DIGEST_BYTES + 8;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A stable SHA-256 digest used for payloads, options, inputs, and cache keys.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Digest([u8; DIGEST_BYTES]);

impl Digest {
    /// Hash arbitrary bytes with the cache's stable SHA-256 implementation.
    pub fn hash(bytes: &[u8]) -> Self {
        Self(sha256(bytes))
    }

    /// Construct a digest from its raw bytes.
    pub const fn from_bytes(bytes: [u8; DIGEST_BYTES]) -> Self {
        Self(bytes)
    }

    /// Return the raw digest bytes.
    pub const fn as_bytes(&self) -> &[u8; DIGEST_BYTES] {
        &self.0
    }

    /// Render the digest as lowercase hexadecimal.
    pub fn to_hex(self) -> String {
        let mut output = String::with_capacity(DIGEST_BYTES * 2);
        for byte in self.0 {
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
        output
    }

    /// Parse a lowercase or uppercase hexadecimal digest.
    pub fn from_hex(value: &str) -> Result<Self, DigestParseError> {
        if value.len() != DIGEST_BYTES * 2 {
            return Err(DigestParseError {
                message: format!("expected {} hexadecimal characters", DIGEST_BYTES * 2),
            });
        }
        let bytes = value.as_bytes();
        let mut digest = [0; DIGEST_BYTES];
        for (index, output) in digest.iter_mut().enumerate() {
            let high = hex_value(bytes[index * 2]).ok_or_else(|| DigestParseError {
                message: format!("invalid hexadecimal character at position {}", index * 2),
            })?;
            let low = hex_value(bytes[index * 2 + 1]).ok_or_else(|| DigestParseError {
                message: format!(
                    "invalid hexadecimal character at position {}",
                    index * 2 + 1
                ),
            })?;
            *output = (high << 4) | low;
        }
        Ok(Self(digest))
    }
}

impl Display for Digest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&(*self).to_hex())
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Digest")
            .field(&self.to_hex())
            .finish()
    }
}

/// An invalid textual digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestParseError {
    message: String,
}

impl Display for DigestParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for DigestParseError {}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

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
    pub fn put(&self, key: &ArtifactKey, payload: &[u8]) -> Result<StoreResult, CacheError> {
        let layer_index = self
            .layers
            .iter()
            .position(CacheLayer::is_writable)
            .ok_or(CacheError::NoWritableLayer)?;
        self.put_in_layer(layer_index, key, payload)
    }

    /// Atomically store an artifact in an explicitly selected writable layer.
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
    match fs::rename(temp_path, destination) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            // Windows does not replace an existing destination with `rename`. A concurrent
            // writer of the same content-addressed key may already have won; accept that record
            // only after fully validating that it is byte-identical to our intended payload.
            let existing = match fs::read(destination) {
                Ok(existing) => existing,
                Err(_) => {
                    return Err(io_error(
                        "install cache artifact",
                        destination,
                        rename_error,
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

// SHA-256, kept local so this foundational crate remains dependency-free.  The implementation
// follows FIPS 180-4's 512-bit block schedule and is intentionally straightforward to audit.
fn sha256(input: &[u8]) -> [u8; DIGEST_BYTES] {
    const INITIAL: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let padded_len = input.len() + 1 + ((64 - ((input.len() + 1 + 8) % 64)) % 64) + 8;
    let mut padded = Vec::with_capacity(padded_len);
    padded.extend_from_slice(input);
    padded.push(0x80);
    padded.resize(padded_len - 8, 0);
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut state = INITIAL;
    for block in padded.chunks_exact(64) {
        let mut schedule = [0u32; 64];
        for (index, word) in schedule[..16].iter_mut().enumerate() {
            let start = index * 4;
            *word = u32::from_be_bytes([
                block[start],
                block[start + 1],
                block[start + 2],
                block[start + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }

        let mut working = state;
        for index in 0..64 {
            let [a, b, c, d, e, f, g, h] = working;
            let big_sigma1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(big_sigma1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(schedule[index]);
            let big_sigma0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = big_sigma0.wrapping_add(majority);
            working = [
                temp1.wrapping_add(temp2),
                a,
                b,
                c,
                d.wrapping_add(temp1),
                e,
                f,
                g,
            ];
        }
        for (slot, value) in state.iter_mut().zip(working) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut output = [0; DIGEST_BYTES];
    for (index, word) in state.iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    output
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
    fn concurrent_puts_leave_one_complete_record_and_no_temporary_files() {
        let root = scratch("atomic");
        let cache = ArtifactCache::new(&root);
        let artifact_key = key("atomic");
        let mut handles = Vec::new();
        for index in 0..8 {
            let cache = cache.clone();
            let artifact_key = artifact_key.clone();
            handles.push(std::thread::spawn(move || {
                let payload = format!("payload-{index}");
                cache
                    .put(&artifact_key, payload.as_bytes())
                    .expect("atomic store succeeds");
            }));
        }
        for handle in handles {
            handle.join().expect("store thread succeeds");
        }

        let Lookup::Hit(hit) = cache.lookup(&artifact_key).expect("lookup succeeds") else {
            panic!("concurrent stores should leave a hit");
        };
        assert!(hit.bytes.starts_with(b"payload-"));
        let parent = hit.path.parent().expect("artifact path has parent");
        let temporary_files = fs::read_dir(parent)
            .expect("artifact parent exists")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(
            temporary_files, 0,
            "atomic store should clean temporary files"
        );
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
