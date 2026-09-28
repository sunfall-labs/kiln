//! The small, versioned ABI between a Kiln program and its host.
//!
//! Compiler and runtime crates may evolve independently of the process that provides host
//! services. Keep values crossing the boundary owned.

use kiln_string::StringUnits;

/// Version of the host operation and value contract.
pub const HOST_ABI_VERSION: u32 = 4;

/// Internal native runtime operation that returns a realm-owned GC callable. It intentionally
/// lives outside [`HostOp`], whose result is the scalar `HostValue` ABI.
pub const HOST_OP_MODULE_CREATE_REQUIRE: u16 = 0x7FF0;

const _: () = {
    assert!(HOST_OP_MODULE_CREATE_REQUIRE > HostOp::TransferRegion as u16);
};

/// Operations a compiled program can request from its host.
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostOp {
    /// Write one line to the host's standard output.
    WriteStdout = 1,
    /// Wall-clock milliseconds since the Unix epoch for a host-clock realm.
    ClockNow = 2,
    /// One host-supplied entropy word for a host-entropy realm.
    EntropyU64 = 3,
    /// The host timezone identifier, such as `America/Denver`.
    TimeZone = 4,
    /// The host locale tag, such as `en-US`.
    Locale = 5,
    /// Write one line to the host's standard error.
    ///
    /// Separate from [`HostOp::WriteStdout`] because the stream a line went to is observable:
    /// `console.error` goes to standard error in every engine a program is written against, and
    /// a caller that redirects one stream and not the other sees the difference. Both engines
    /// shared the stdout operation until this existed, which made `2>&1` the only way their
    /// output matched node's.
    WriteStderr = 6,
    /// Transfer an isolated object graph or memory region to another realm (ADR 0057).
    TransferRegion = 7,
}

/// A scalar value that is safe to own at the host boundary.
///
/// Numbers are represented by their IEEE-754 bits. This preserves NaN payloads and negative
/// zero while keeping equality total and making serialization the responsibility of a future
/// ABI transport rather than this foundational crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(u64),
    String(StringUnits),
}

impl HostValue {
    pub const fn number(value: f64) -> Self {
        Self::Number(value.to_bits())
    }

    pub const fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }

    /// JavaScript's scalar display form, suitable for `console.log`-style output.
    /// The value as a console writes it.
    ///
    /// Differs from [`Self::to_js_string`] only for negative zero, which `String(-0)` renders as
    /// `0` and a console renders as `-0` -- the console is showing the value, not converting it.
    pub fn to_console_string(&self) -> String {
        match self {
            Self::Number(bits) => {
                let value = f64::from_bits(*bits);
                if value == 0.0 && value.is_sign_negative() {
                    return "-0".to_owned();
                }
                number_to_js_string(value)
            }
            other => other.to_js_string(),
        }
    }

    pub fn to_js_string(&self) -> String {
        match self {
            Self::Undefined => "undefined".to_owned(),
            Self::Null => "null".to_owned(),
            Self::Boolean(value) => value.to_string(),
            Self::Number(bits) => number_to_js_string(f64::from_bits(*bits)),
            Self::String(value) => value.to_lossy_string(),
        }
    }
}

/// Opaque identity of one asynchronous host request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(u64);

impl RequestId {
    /// Creates an identity from its stable numeric representation.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric representation used by private embedding seams.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Opaque identity of a host resource owned by one realm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(u64);

impl ResourceId {
    /// Creates an identity from its stable numeric representation.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric representation used by private embedding seams.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One asynchronous operation requested from a realm host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsyncHostOp {
    /// Read an entire file as owned bytes.
    ReadFile { path: String },
    /// Replace or create a file with the supplied bytes.
    WriteFile { path: String, data: Vec<u8> },
    /// Append bytes to a file, creating it if it does not exist.
    AppendFile { path: String, data: Vec<u8> },
    /// Read metadata without opening a persistent file resource.
    Stat { path: String },
    /// Read metadata without following symbolic links.
    Lstat { path: String },
    /// Create a directory, optionally recursively.
    Mkdir { path: String, recursive: bool },
    /// Read directory entries.
    ReadDir { path: String, with_types: bool },
    /// Copy a file from source to destination.
    CopyFile { from: String, to: String },
    /// Rename a file or directory.
    Rename { from: String, to: String },
    /// Remove a file.
    Unlink { path: String },
    /// Remove a file or directory with options.
    Rm {
        path: String,
        recursive: bool,
        force: bool,
    },
    /// Resolve canonical path.
    Realpath { path: String },
    /// Read link target.
    Readlink { path: String },
    /// Test file accessibility.
    Access { path: String, mode: u32 },
    /// Change file permissions.
    Chmod { path: String, mode: u32 },
    /// Remove a directory.
    Rmdir { path: String },
    /// Close a persistent host resource.
    CloseResource { resource: ResourceId },
    /// Change whether a persistent resource keeps its realm alive.
    SetResourceReferenced {
        resource: ResourceId,
        referenced: bool,
    },
    /// Inspect lifecycle state without exposing the host's resource-table representation.
    ResourceState { resource: ResourceId },
    /// Bind a TCP listener. Port zero asks the host to select an ephemeral port.
    TcpListen { host: String, port: u16 },
    /// Wait for one connection and return its stream resource.
    TcpAccept { listener: ResourceId },
    /// Connect a TCP stream.
    TcpConnect { host: String, port: u16 },
    /// Read up to `max_bytes` from a stream. Empty bytes represent EOF.
    TcpRead {
        stream: ResourceId,
        max_bytes: usize,
    },
    /// Write the complete byte buffer to a stream.
    TcpWrite { stream: ResourceId, data: Vec<u8> },
    /// Shut down the write half of a stream.
    TcpShutdown { stream: ResourceId },
    /// Read up to `max_bytes` from the host's standard input. Empty bytes represent EOF.
    StdinRead { max_bytes: usize },
    /// Run one child process to completion, optionally writing bytes to its standard input.
    ProcessRun {
        command: String,
        args: Vec<String>,
        stdin: Option<Vec<u8>>,
    },
    /// Wait until a cross-realm message port (`kiln_runtime::workers`) has a message queued, or
    /// its peer has closed. Completes with no payload: the realm then takes what is queued.
    PortWait { port: u64 },
    /// Wait on a registered shared-memory cell without occupying an executor lane. The ticket
    /// was compared and enqueued under the process waiter lock before this request was started.
    AtomicsWait {
        ticket: u64,
        timeout: Option<std::time::Duration>,
    },
}

/// Stable resource category that does not expose executor or operating-system types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    TcpListener,
    TcpStream,
}

/// Portable information returned when the host creates a persistent resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceInfo {
    pub id: ResourceId,
    pub kind: ResourceKind,
    pub address: Option<String>,
    pub port: Option<u16>,
}

/// Observable lifecycle state of a realm-owned resource.
///
/// Node adapters use this snapshot to implement `ref`, `unref`, `hasRef`, half-close, and
/// diagnostics without learning which executor or operating-system handle backs the resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceState {
    pub info: ResourceInfo,
    pub referenced: bool,
    /// True once the read half has observed EOF. Listeners do not have a read half.
    pub read_closed: bool,
    /// True once the write half has been shut down. Listeners do not have a write half.
    pub write_closed: bool,
}

/// Result of accepting a TCP connection, including both local and peer endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedTcpStream {
    pub resource: ResourceInfo,
    pub peer_address: String,
    pub peer_port: u16,
}

/// Portable file metadata needed by Node filesystem slices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    /// File length in bytes.
    pub size: u64,
    /// Whether the path names a regular file.
    pub is_file: bool,
    /// Whether the path names a directory.
    pub is_directory: bool,
    /// Whether the path names a symbolic link.
    pub is_symlink: bool,
}

/// Portable directory entry returned by directory reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_file: bool,
    pub is_directory: bool,
    pub is_symlink: bool,
}

/// A successful asynchronous host operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsyncHostValue {
    /// No payload beyond successful completion.
    Undefined,
    /// Owned bytes, normally a file or stream chunk.
    Bytes(Vec<u8>),
    /// Portable filesystem metadata.
    FileStat(FileStat),
    /// Directory entries returned by readdir.
    DirEntries(Vec<DirEntry>),
    /// String returned by realpath or readlink.
    String(String),
    /// A persistent resource inserted into the realm's resource table.
    Resource(ResourceInfo),
    /// Current lifecycle state of a persistent resource.
    ResourceState(ResourceState),
    /// A newly accepted TCP stream.
    AcceptedTcpStream(AcceptedTcpStream),
    /// Captured child-process output and termination status.
    ProcessOutput(ProcessOutput),
}

/// Owned output from a completed child process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    /// Platform signal number when the process was terminated by a signal.
    pub signal: Option<i32>,
}

/// Stable error information that a Node compatibility Adapter can turn into a JavaScript error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsyncHostError {
    /// Node-compatible short error code where one exists, such as `ENOENT`.
    pub code: String,
    /// Operation name, such as `open` or `stat`.
    pub syscall: String,
    /// Path associated with the failure, if any.
    pub path: Option<String>,
    /// Human-readable detail supplied by the host implementation.
    pub message: String,
}

impl AsyncHostError {
    /// Constructs one structured asynchronous host error.
    pub fn new(
        code: impl Into<String>,
        syscall: impl Into<String>,
        path: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            syscall: syscall.into(),
            path,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AsyncHostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AsyncHostError {}

/// Result delivered when an asynchronous host request reaches a realm checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// The request being completed.
    pub request: RequestId,
    /// Monotonic submission order used as the deterministic readiness tie-break.
    pub sequence: u64,
    /// Operation result owned by the receiving realm.
    pub result: Result<AsyncHostValue, AsyncHostError>,
}

/// Starts and controls asynchronous operations without exposing an executor implementation.
pub trait AsyncHostOps {
    /// Start a non-resource operation and return its opaque request identity immediately.
    ///
    /// Persistent-resource operations are started by the runtime owner so creation and table
    /// mutation stay on the realm task; this trait remains the provider seam used by filesystem
    /// operations and deterministic scripted hosts.
    fn start(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId;

    /// Cancel a pending operation, returning whether it was still owned by this host.
    fn cancel(&mut self, request: RequestId) -> bool;

    /// Return whether referenced work currently keeps the realm alive.
    fn has_referenced_work(&self) -> bool;

    /// Block until the next completion is ready, or return `None` when no requests remain.
    fn next_completion(&mut self) -> Option<Completion>;
}

/// Starts and cancels asynchronous services while code is executing on its realm task.
///
/// This is the narrow active-realm half of [`AsyncHostOps`]. It deliberately has no blocking
/// completion method: callers already inside a runtime must return control to the realm pump before
/// a provider can make progress. Persistent-resource operations use this same service boundary;
/// they are not a Node-specific executor API.
pub trait ActiveAsyncHostOps {
    /// Submit one operation and return its realm-scoped request identity immediately.
    fn start(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId;

    /// Cancel a pending operation, returning whether it was still owned by this host.
    fn cancel(&mut self, request: RequestId) -> bool;
}

/// A number as `String(value)` renders it.
///
/// The allocation-free semantic core writes into caller-owned storage; this boundary turns those
/// bytes into the owned string expected by interpreter hosts. Keeping the digit algorithm out of
/// this crate also keeps interpreter and native artifacts on one implementation.
pub fn number_to_js_string(value: f64) -> String {
    let mut output = [0_u8; kiln_number_core::MAX_FORMATTED_LEN];
    let Ok(length) = kiln_number_core::format_number(value, &mut output) else {
        // `MAX_FORMATTED_LEN` is the semantic core's own advertised bound. Reaching this branch
        // means that crate violated its API invariant; avoid converting that into a host panic.
        return "NaN".to_owned();
    };
    let Some(bytes) = output.get(..length) else {
        return "NaN".to_owned();
    };
    String::from_utf8_lossy(bytes).into_owned()
}

/// Converts a string using the ECMAScript `StringNumericLiteral` grammar.
///
/// The semantic core consumes a bounded byte span so the same operation can cross the private C
/// runtime seam without allocation or NUL-termination assumptions.
pub fn string_to_number(value: &str) -> f64 {
    kiln_number_core::string_to_number(value.as_bytes())
}

/// An operation failure reported by a host implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostError {
    pub message: String,
}

impl HostError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HostError {}

/// Name of the transfer region host operation symbol in emitted C and JavaScript.
pub const TRANSFER_REGION_HOST_OP_NAME: &str = "__kiln_transfer_region";

/// Evaluates region transfer for an opaque pointer representation across host boundaries.
pub fn transfer_region_op(ptr: u64) -> Result<HostValue, HostError> {
    Ok(HostValue::Number(ptr))
}

/// Host implementation invoked by the interpreter or a future native backend.
pub trait HostOps {
    fn call(&mut self, operation: HostOp, arguments: &[HostValue]) -> Result<HostValue, HostError>;

    /// Optional synchronous filesystem capability.
    fn fs(&mut self) -> Option<&mut (dyn FileSystemOps + '_)> {
        None
    }

    /// Clone this host's filesystem capability, if present.
    fn clone_fs(&self) -> Option<Box<dyn FileSystemOps>> {
        None
    }
}

/// Portable metadata about a filesystem entry, usable by both OS and virtual filesystems.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsMetadata {
    pub len: u64,
    pub is_file: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub modified_ms: u64,
    pub created_ms: u64,
    pub mode: u32,
}

impl FsMetadata {
    pub fn file(len: u64, modified_ms: u64) -> Self {
        Self {
            len,
            is_file: true,
            is_dir: false,
            is_symlink: false,
            modified_ms,
            created_ms: modified_ms,
            mode: 0o644,
        }
    }

    pub fn dir(modified_ms: u64) -> Self {
        Self {
            len: 0,
            is_file: false,
            is_dir: true,
            is_symlink: false,
            modified_ms,
            created_ms: modified_ms,
            mode: 0o755,
        }
    }
}

impl From<&std::fs::Metadata> for FsMetadata {
    fn from(meta: &std::fs::Metadata) -> Self {
        let is_file = meta.is_file();
        let is_dir = meta.is_dir();
        let is_symlink = meta.file_type().is_symlink();
        let len = meta.len();
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        let created_ms = meta
            .created()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(modified_ms, |d| d.as_millis() as u64);
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode()
        };
        #[cfg(not(unix))]
        let mode = 0;
        Self {
            len,
            is_file,
            is_dir,
            is_symlink,
            modified_ms,
            created_ms,
            mode,
        }
    }
}

/// Synchronous filesystem operations provided by a host capability.
pub trait FileSystemOps {
    fn exists(&mut self, path: &std::path::Path) -> bool;
    fn read(&mut self, path: &std::path::Path) -> std::io::Result<Vec<u8>>;
    fn write(&mut self, path: &std::path::Path, data: &[u8]) -> std::io::Result<()>;
    fn append(&mut self, path: &std::path::Path, data: &[u8]) -> std::io::Result<()>;
    fn metadata(&mut self, path: &std::path::Path) -> std::io::Result<FsMetadata>;
    fn symlink_metadata(&mut self, path: &std::path::Path) -> std::io::Result<FsMetadata>;
    fn create_dir(&mut self, path: &std::path::Path) -> std::io::Result<()>;
    fn create_dir_all(&mut self, path: &std::path::Path) -> std::io::Result<()>;
    fn remove_file(&mut self, path: &std::path::Path) -> std::io::Result<()>;
    fn remove_dir(&mut self, path: &std::path::Path) -> std::io::Result<()>;
    fn remove_dir_all(&mut self, path: &std::path::Path) -> std::io::Result<()>;
    fn read_dir(&mut self, path: &std::path::Path) -> std::io::Result<Vec<DirEntry>>;
    fn canonicalize(&mut self, path: &std::path::Path) -> std::io::Result<std::path::PathBuf>;
    fn rename(&mut self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()>;
    fn copy(&mut self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<u64>;
    fn set_permissions(&mut self, path: &std::path::Path, mode: u32) -> std::io::Result<()>;
    fn read_link(&mut self, path: &std::path::Path) -> std::io::Result<std::path::PathBuf>;

    /// Returns true if the path exists in an in-memory overlay table without syscalls.
    fn is_in_memory(&self, _path: &std::path::Path) -> bool {
        false
    }

    /// Read from an in-memory overlay table without syscalls, if present.
    fn read_in_memory(&self, _path: &std::path::Path) -> Option<Vec<u8>> {
        None
    }

    /// Fast-path stat metadata from an in-memory overlay table without syscalls, if present.
    fn metadata_in_memory(&self, _path: &std::path::Path) -> Option<FsMetadata> {
        None
    }

    /// Fast-path directory entries from an in-memory overlay table without syscalls, if present.
    fn read_dir_in_memory(&self, _path: &std::path::Path) -> Option<Vec<DirEntry>> {
        None
    }

    /// Clone this filesystem capability if supported.
    fn clone_box(&self) -> Option<Box<dyn FileSystemOps>> {
        None
    }
}

/// Standard operating-system filesystem implementation backed by `std::fs`.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsFileSystem;

impl FileSystemOps for OsFileSystem {
    fn exists(&mut self, path: &std::path::Path) -> bool {
        path.exists()
    }

    fn read(&mut self, path: &std::path::Path) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write(&mut self, path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
        std::fs::write(path, data)
    }

    fn append(&mut self, path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(data)
    }

    fn metadata(&mut self, path: &std::path::Path) -> std::io::Result<FsMetadata> {
        std::fs::metadata(path).map(|m| FsMetadata::from(&m))
    }

    fn symlink_metadata(&mut self, path: &std::path::Path) -> std::io::Result<FsMetadata> {
        std::fs::symlink_metadata(path).map(|m| FsMetadata::from(&m))
    }

    fn create_dir(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir(path)
    }

    fn create_dir_all(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn remove_file(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::remove_file(path)
    }

    fn remove_dir(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::remove_dir(path)
    }

    fn remove_dir_all(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::remove_dir_all(path)
    }

    fn read_dir(&mut self, path: &std::path::Path) -> std::io::Result<Vec<DirEntry>> {
        let rd = std::fs::read_dir(path)?;
        let mut entries = Vec::new();
        for entry in rd {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let ft = entry.file_type().ok();
            entries.push(DirEntry {
                name,
                is_file: ft.as_ref().map_or(false, |t| t.is_file()),
                is_directory: ft.as_ref().map_or(false, |t| t.is_dir()),
                is_symlink: ft.as_ref().map_or(false, |t| t.is_symlink()),
            });
        }
        Ok(entries)
    }

    fn canonicalize(&mut self, path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
        std::fs::canonicalize(path)
    }

    fn rename(&mut self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }

    fn copy(&mut self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<u64> {
        std::fs::copy(from, to)
    }

    fn set_permissions(&mut self, path: &std::path::Path, mode: u32) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        }
        #[cfg(not(unix))]
        {
            let _ = (path, mode);
            Ok(())
        }
    }

    fn read_link(&mut self, path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
        std::fs::read_link(path)
    }

    fn clone_box(&self) -> Option<Box<dyn FileSystemOps>> {
        Some(Box::new(OsFileSystem))
    }
}

/// An in-memory filesystem overlay on top of a fallback filesystem provider.
///
/// In-memory files are served directly with zero syscalls. Fallback handles misses and modifications.
#[derive(Debug, Clone)]
pub struct OverlayFileSystem<F = OsFileSystem> {
    fallback: F,
    files:
        std::sync::Arc<std::sync::RwLock<std::collections::HashMap<std::path::PathBuf, Vec<u8>>>>,
}

impl<F: FileSystemOps> OverlayFileSystem<F> {
    pub fn new(fallback: F, files: std::collections::HashMap<std::path::PathBuf, Vec<u8>>) -> Self {
        Self {
            fallback,
            files: std::sync::Arc::new(std::sync::RwLock::new(files)),
        }
    }

    pub fn insert(&mut self, path: impl Into<std::path::PathBuf>, data: Vec<u8>) {
        self.files.write().unwrap().insert(path.into(), data);
    }

    fn lookup(&self, path: &std::path::Path) -> Option<Vec<u8>> {
        let files = self.files.read().unwrap();
        if let Some(bytes) = files.get(path) {
            return Some(bytes.clone());
        }
        if let Ok(stripped) = path.strip_prefix(".") {
            if let Some(bytes) = files.get(stripped) {
                return Some(bytes.clone());
            }
        }
        None
    }

    fn is_in_memory_dir(&self, path: &std::path::Path) -> bool {
        let files = self.files.read().unwrap();
        files.keys().any(|k| {
            if let Ok(rel) = k.strip_prefix(path) {
                rel != std::path::Path::new("")
            } else if let Ok(stripped) = k.strip_prefix(".") {
                if let Ok(rel) = stripped.strip_prefix(path) {
                    rel != std::path::Path::new("")
                } else {
                    false
                }
            } else {
                false
            }
        })
    }
}

impl<F: FileSystemOps + Clone + 'static> FileSystemOps for OverlayFileSystem<F> {
    fn exists(&mut self, path: &std::path::Path) -> bool {
        if self.lookup(path).is_some() || self.is_in_memory_dir(path) {
            true
        } else {
            self.fallback.exists(path)
        }
    }

    fn read(&mut self, path: &std::path::Path) -> std::io::Result<Vec<u8>> {
        if let Some(bytes) = self.lookup(path) {
            Ok(bytes)
        } else {
            self.fallback.read(path)
        }
    }

    fn write(&mut self, path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
        self.files
            .write()
            .unwrap()
            .insert(path.to_path_buf(), data.to_vec());
        self.fallback.write(path, data)
    }

    fn append(&mut self, path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
        let mut files = self.files.write().unwrap();
        if let Some(existing) = files.get_mut(path) {
            existing.extend_from_slice(data);
        } else {
            files.insert(path.to_path_buf(), data.to_vec());
        }
        drop(files);
        self.fallback.append(path, data)
    }

    fn metadata(&mut self, path: &std::path::Path) -> std::io::Result<FsMetadata> {
        if let Some(bytes) = self.lookup(path) {
            Ok(FsMetadata::file(bytes.len() as u64, 0))
        } else if self.is_in_memory_dir(path) && !self.fallback.exists(path) {
            Ok(FsMetadata::dir(0))
        } else {
            self.fallback.metadata(path)
        }
    }

    fn symlink_metadata(&mut self, path: &std::path::Path) -> std::io::Result<FsMetadata> {
        if let Some(bytes) = self.lookup(path) {
            Ok(FsMetadata::file(bytes.len() as u64, 0))
        } else if self.is_in_memory_dir(path) && !self.fallback.exists(path) {
            Ok(FsMetadata::dir(0))
        } else {
            self.fallback.symlink_metadata(path)
        }
    }

    fn create_dir(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.fallback.create_dir(path)
    }

    fn create_dir_all(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.fallback.create_dir_all(path)
    }

    fn remove_file(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        let mut files = self.files.write().unwrap();
        files.remove(path);
        if let Ok(stripped) = path.strip_prefix(".") {
            files.remove(stripped);
        }
        drop(files);
        self.fallback.remove_file(path)
    }

    fn remove_dir(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.fallback.remove_dir(path)
    }

    fn remove_dir_all(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        // `write` records every file here as well as on disk, so a directory removed on disk
        // alone still "exists" through `is_in_memory_dir`: `fs.rmSync(dir, { recursive: true })`
        // followed by `fs.existsSync(dir)` answered true. The overlay forgets what was under it.
        {
            let mut files = self.files.write().unwrap();
            files.retain(|k, _| {
                !(k.starts_with(path)
                    || k.strip_prefix(".")
                        .map_or(false, |stripped| stripped.starts_with(path)))
            });
        }
        self.fallback.remove_dir_all(path)
    }

    fn read_dir(&mut self, path: &std::path::Path) -> std::io::Result<Vec<DirEntry>> {
        let mut entries = self.fallback.read_dir(path).unwrap_or_default();
        let mut seen: std::collections::HashSet<String> =
            entries.iter().map(|e| e.name.clone()).collect();
        let files = self.files.read().unwrap();
        for file_path in files.keys() {
            let parent = file_path.parent();
            if parent == Some(path) || (parent.is_none() && path == std::path::Path::new("")) {
                if let Some(name) = file_path.file_name().and_then(|n| n.to_str()) {
                    if seen.insert(name.to_owned()) {
                        entries.push(DirEntry {
                            name: name.to_owned(),
                            is_file: true,
                            is_directory: false,
                            is_symlink: false,
                        });
                    }
                }
            }
        }
        Ok(entries)
    }

    fn canonicalize(&mut self, path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
        if self.lookup(path).is_some() || self.is_in_memory_dir(path) {
            Ok(path.to_path_buf())
        } else {
            self.fallback.canonicalize(path)
        }
    }

    fn rename(&mut self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        let mut files = self.files.write().unwrap();
        if let Some(bytes) = files.remove(from) {
            files.insert(to.to_path_buf(), bytes);
        }
        drop(files);
        self.fallback.rename(from, to)
    }

    fn copy(&mut self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<u64> {
        let maybe_bytes = self.lookup(from);
        if let Some(bytes) = maybe_bytes {
            let len = bytes.len() as u64;
            let _ = self.fallback.write(to, &bytes);
            self.files.write().unwrap().insert(to.to_path_buf(), bytes);
            Ok(len)
        } else {
            self.fallback.copy(from, to)
        }
    }

    fn set_permissions(&mut self, path: &std::path::Path, mode: u32) -> std::io::Result<()> {
        self.fallback.set_permissions(path, mode)
    }

    fn read_link(&mut self, path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
        self.fallback.read_link(path)
    }

    fn is_in_memory(&self, path: &std::path::Path) -> bool {
        self.lookup(path).is_some() || self.is_in_memory_dir(path)
    }

    fn read_in_memory(&self, path: &std::path::Path) -> Option<Vec<u8>> {
        self.lookup(path)
    }

    fn metadata_in_memory(&self, path: &std::path::Path) -> Option<FsMetadata> {
        if let Some(bytes) = self.lookup(path) {
            Some(FsMetadata::file(bytes.len() as u64, 0))
        } else if self.is_in_memory_dir(path) {
            Some(FsMetadata::dir(0))
        } else {
            None
        }
    }

    fn read_dir_in_memory(&self, path: &std::path::Path) -> Option<Vec<DirEntry>> {
        if self.is_in_memory_dir(path) {
            let files = self.files.read().unwrap();
            let mut entries = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for file_path in files.keys() {
                let parent = file_path.parent();
                if parent == Some(path) || (parent.is_none() && path == std::path::Path::new("")) {
                    if let Some(name) = file_path.file_name().and_then(|n| n.to_str()) {
                        if seen.insert(name.to_owned()) {
                            entries.push(DirEntry {
                                name: name.to_owned(),
                                is_file: true,
                                is_directory: false,
                                is_symlink: false,
                            });
                        }
                    }
                }
            }
            Some(entries)
        } else {
            None
        }
    }

    fn clone_box(&self) -> Option<Box<dyn FileSystemOps>> {
        Some(Box::new(self.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    #[test]
    fn numbers_round_trip_as_bits() {
        let value = HostValue::number(-0.0);
        assert_eq!(value, HostValue::Number((-0.0_f64).to_bits()));
        assert_eq!(value.as_number(), Some(-0.0));
        assert_eq!(value.to_js_string(), "0");
    }

    #[test]
    fn scalar_display_is_js_like() {
        assert_eq!(HostValue::Undefined.to_js_string(), "undefined");
        assert_eq!(HostValue::Null.to_js_string(), "null");
        assert_eq!(HostValue::Boolean(true).to_js_string(), "true");
        assert_eq!(HostValue::number(f64::NAN).to_js_string(), "NaN");
        assert_eq!(HostValue::number(f64::INFINITY).to_js_string(), "Infinity");
        assert_eq!(HostValue::String("kiln".into()).to_js_string(), "kiln");
    }

    #[test]
    fn async_identities_round_trip_without_exposing_layout() {
        assert_eq!(RequestId::new(7).get(), 7);
        assert_eq!(ResourceId::new(11).get(), 11);
    }

    #[test]
    fn async_errors_keep_node_shaping_data_separate() {
        let error = AsyncHostError::new(
            "ENOENT",
            "open",
            Some("missing.txt".to_owned()),
            "no such file or directory",
        );
        assert_eq!(error.code, "ENOENT");
        assert_eq!(error.syscall, "open");
        assert_eq!(error.path.as_deref(), Some("missing.txt"));
        assert_eq!(error.to_string(), "ENOENT: no such file or directory");
    }

    #[test]
    fn numbers_switch_to_exponential_where_javascript_does() {
        // The two boundaries, from both sides. `f64::to_string` crosses neither, which is the
        // whole reason this function exists.
        assert_eq!(number_to_js_string(1e20), "100000000000000000000");
        assert_eq!(number_to_js_string(1e21), "1e+21");
        assert_eq!(number_to_js_string(1e-6), "0.000001");
        assert_eq!(number_to_js_string(1e-7), "1e-7");
        assert_eq!(number_to_js_string(1.5e21), "1.5e+21");
        assert_eq!(number_to_js_string(1.2345e-7), "1.2345e-7");
        assert_eq!(number_to_js_string(-1e21), "-1e+21");
        assert_eq!(number_to_js_string(5e-324), "5e-324");
    }

    #[test]
    fn ordinary_numbers_keep_their_shortest_form() {
        assert_eq!(number_to_js_string(100.0), "100");
        assert_eq!(number_to_js_string(1.5), "1.5");
        assert_eq!(number_to_js_string(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(number_to_js_string(1.0 / 3.0), "0.3333333333333333");
        assert_eq!(number_to_js_string(2f64.powi(53)), "9007199254740992");
        // Rust's scientific formatter chooses a different last digit here. ECMAScript requires
        // the shortest decimal which round-trips to the same binary64 value.
        assert_eq!(
            number_to_js_string(157_651_540_651_918.12),
            "157651540651918.12"
        );
    }

    #[test]
    fn the_non_finite_values_have_names_rather_than_digits() {
        assert_eq!(number_to_js_string(f64::NAN), "NaN");
        assert_eq!(number_to_js_string(f64::INFINITY), "Infinity");
        assert_eq!(number_to_js_string(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn only_a_console_distinguishes_negative_zero() {
        // `String(-0)` is "0" and a console shows "-0": converting and displaying are different
        // questions, and the two methods are where that difference lives.
        assert_eq!(number_to_js_string(-0.0), "0");
        assert_eq!(HostValue::number(-0.0).to_js_string(), "0");
        assert_eq!(HostValue::number(-0.0).to_console_string(), "-0");
        assert_eq!(HostValue::number(0.0).to_console_string(), "0");
        // Everything else is the same through both.
        assert_eq!(HostValue::number(1.5).to_console_string(), "1.5");
    }

    #[test]
    fn string_numbers_accept_the_decimal_grammar() {
        assert_eq!(string_to_number("0"), 0.0);
        assert_eq!(string_to_number("+12"), 12.0);
        assert_eq!(string_to_number("-12."), -12.0);
        assert_eq!(string_to_number(".125"), 0.125);
        assert_eq!(string_to_number("12.5e-1"), 1.25);
        assert_eq!(string_to_number("1E+3"), 1000.0);
        assert_eq!(string_to_number("Infinity"), f64::INFINITY);
        assert_eq!(string_to_number("+Infinity"), f64::INFINITY);
        assert_eq!(string_to_number("-Infinity"), f64::NEG_INFINITY);
    }

    #[test]
    fn string_numbers_trim_only_ecmascript_whitespace() {
        let whitespace = "\u{0009}\u{000A}\u{000B}\u{000C}\u{000D}\u{0020}\u{00A0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}";
        assert_eq!(
            string_to_number(&format!("{whitespace}42{whitespace}")),
            42.0
        );
        assert!(string_to_number("\u{180E}42").is_nan());
        assert!(string_to_number("4\u{0085}2").is_nan());
    }

    #[test]
    fn string_numbers_reject_non_literals() {
        for value in [
            "+", "-", ".", ".e1", "1e", "1e+", "1..0", "1 2", "inf", "-inf", "nan", "NaN", "0x",
            "0o", "0b", "0x1g", "0o8", "0b2", "+0x1", "-0b1",
        ] {
            assert!(
                string_to_number(value).is_nan(),
                "accepted invalid literal {value:?}"
            );
        }
    }

    #[test]
    fn string_numbers_preserve_negative_zero_and_empty_is_positive_zero() {
        assert_eq!(string_to_number(""), 0.0);
        assert!(!string_to_number("").is_sign_negative());
        assert!(string_to_number("-0").is_sign_negative());
        assert!(string_to_number("-0.0e+100").is_sign_negative());
    }

    #[test]
    fn string_numbers_parse_unsigned_radix_forms() {
        assert_eq!(string_to_number("0b101010"), 42.0);
        assert_eq!(string_to_number("0o755"), 493.0);
        assert_eq!(string_to_number("0xDeadBeef"), 3_735_928_559.0);
        assert_eq!(string_to_number("0x0000000000000001"), 1.0);
        assert_eq!(
            string_to_number(&("0b".to_owned() + &"1".repeat(200))),
            2_f64.powi(200)
        );
    }

    #[test]
    fn string_numbers_radix_rounding_is_ties_to_even() {
        // At this magnitude the spacing is two.  +1 is exactly halfway and rounds down to the
        // even significand; +3 is halfway between the next pair and rounds up.
        assert_eq!(
            string_to_number("0x20000000000001"),
            9_007_199_254_740_992.0
        );
        assert_eq!(
            string_to_number("0x20000000000003"),
            9_007_199_254_740_996.0
        );
        assert_eq!(
            string_to_number("0x20000000000002"),
            9_007_199_254_740_994.0
        );

        // The same check through binary and octal digit boundaries exercises leading-bit removal.
        assert_eq!(
            string_to_number(&format!("0b1{}1", "0".repeat(52))),
            2_f64.powi(53)
        );
        assert_eq!(
            string_to_number(&format!("0o4{}1", "0".repeat(16))),
            2_f64.powi(53)
        );
    }

    #[test]
    fn string_numbers_radix_overflow_to_infinity() {
        let max_finite_bits = format!("0b{}{}", "1".repeat(53), "0".repeat(971));
        assert_eq!(
            string_to_number(&max_finite_bits),
            f64::from_bits(0x7fefffffffffffff)
        );
        let overflow_tie = format!("0b{}1{}", "1".repeat(53), "0".repeat(970));
        assert_eq!(string_to_number(&overflow_tie), f64::INFINITY);
        assert_eq!(
            string_to_number(&format!("0x1{}", "0".repeat(256))),
            f64::INFINITY
        );
        assert_eq!(
            string_to_number(&format!("0b1{}", "0".repeat(1024))),
            f64::INFINITY
        );
        assert_eq!(
            string_to_number("0xffffffffffffffff"),
            18_446_744_073_709_551_616.0
        );
    }

    #[test]
    fn overlay_filesystem_intercepts_memory_files() {
        let mut overlay_map = HashMap::new();
        overlay_map.insert(PathBuf::from("virtual.txt"), b"virtual content".to_vec());
        let mut vfs = OverlayFileSystem::new(OsFileSystem, overlay_map);

        assert!(vfs.exists(Path::new("virtual.txt")));
        assert!(vfs.is_in_memory(Path::new("virtual.txt")));
        assert_eq!(
            vfs.read_in_memory(Path::new("virtual.txt")).unwrap(),
            b"virtual content"
        );
        let in_mem_meta = vfs.metadata_in_memory(Path::new("virtual.txt")).unwrap();
        assert!(in_mem_meta.is_file);
        assert_eq!(in_mem_meta.len, 15);

        assert_eq!(
            vfs.read(Path::new("virtual.txt")).unwrap(),
            b"virtual content"
        );
        let meta = vfs.metadata(Path::new("virtual.txt")).unwrap();
        assert!(meta.is_file);
        assert_eq!(meta.len, 15);

        let cloned = vfs.clone_box().unwrap();
        assert!(cloned.is_in_memory(Path::new("virtual.txt")));
        assert_eq!(
            cloned.read_in_memory(Path::new("virtual.txt")).unwrap(),
            b"virtual content"
        );
    }
}
