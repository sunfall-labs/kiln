//! Discovery and invocation of a C toolchain used by Kiln's native backend.
//!
//! The native backend emits C; this crate owns the small amount of platform-facing behavior needed
//! to turn those translation units into objects, executables, or shared libraries. It deliberately
//! does not know how C is generated or how a runtime is laid out. Callers provide paths and
//! optional runtime libraries, while the crate provides:
//!
//! * deterministic compiler discovery (`zig cc`, Clang, GCC, or a generic `cc`),
//! * native-dev/native-check/native-release flags,
//! * captured process output and structured failures, and
//! * per-artifact timing, size, and compiler identity.
//!
//! [`ProcessRunner`] is injectable so tests and a future daemon can exercise command construction
//! without starting a child process.  The default runner uses [`std::process::Command`] with both
//! output streams piped and stdin disconnected.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Compilers considered by [`Toolchain::discover`], in preference order.
pub const DEFAULT_COMPILER_CANDIDATES: &[&str] = &["zig", "clang", "cc", "gcc"];

/// The broad driver family selected for a compiler executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompilerKind {
    /// Zig's C frontend, invoked as `zig cc`.
    Zig,
    /// Clang or a Clang-compatible driver.
    Clang,
    /// GCC or a GCC-compatible driver.
    Gcc,
    /// A generic `cc` driver whose family is not otherwise known.
    Cc,
}

/// The operating-system family targeted by a C compiler.
///
/// This is derived from the compiler's reported target triple rather than Rust's compilation
/// target.  In particular, a compiler running on macOS can target Linux, and a compiler running
/// on Linux can target macOS.  `Unknown` means the compiler did not provide a target probe, while
/// `Other` is a target triple that was reported successfully but is not one of the platform
/// families currently understood by Kiln.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetPlatform {
    Macos,
    Linux,
    Windows,
    Other,
    Unknown,
}

impl TargetPlatform {
    /// Classify a compiler-reported target triple.
    pub fn from_target(target: &str) -> Self {
        let target = target.trim();
        if target.is_empty() || target.eq_ignore_ascii_case("unknown") {
            return Self::Unknown;
        }

        // Target triples use `-` as their primary separator, but accepting `_` and `.` here
        // keeps this classifier useful for Zig's shorter aliases and versioned Darwin triples.
        let components = target
            .to_ascii_lowercase()
            .split(['-', '_', '.'])
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let starts_with = |prefix: &str| {
            components
                .iter()
                .any(|component| component.starts_with(prefix))
        };
        if starts_with("darwin") || starts_with("macos") {
            Self::Macos
        } else if starts_with("linux") {
            Self::Linux
        } else if starts_with("windows") || starts_with("mingw") || starts_with("msvc") {
            Self::Windows
        } else {
            Self::Other
        }
    }

    /// Shared-library filename suffix without a leading dot.
    pub const fn shared_library_suffix(self) -> Option<&'static str> {
        match self {
            Self::Macos => Some("dylib"),
            Self::Linux => Some("so"),
            Self::Windows => Some("dll"),
            Self::Other | Self::Unknown => None,
        }
    }

    /// Default temporary shared-library filename for this target family.
    pub const fn shared_library_name(self) -> Option<&'static str> {
        match self {
            Self::Macos => Some("program.dylib"),
            Self::Linux => Some("program.so"),
            Self::Windows => Some("program.dll"),
            Self::Other | Self::Unknown => None,
        }
    }

    /// Executable filename suffix for this target family.
    pub const fn executable_suffix(self) -> &'static str {
        match self {
            Self::Windows => ".exe",
            Self::Macos | Self::Linux | Self::Other | Self::Unknown => "",
        }
    }

    /// System math library required by generated C for this target family.
    pub const fn system_math_library(self) -> Option<&'static str> {
        match self {
            Self::Macos | Self::Linux => Some("m"),
            Self::Windows | Self::Other | Self::Unknown => None,
        }
    }

    /// Linker mode flags for a shared library, if this target family is supported.
    pub const fn shared_link_flags(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Macos => Some(&["-dynamiclib"]),
            Self::Linux => Some(&["-shared"]),
            Self::Windows | Self::Other | Self::Unknown => None,
        }
    }
}

impl fmt::Display for CompilerKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Zig => "zig",
            Self::Clang => "clang",
            Self::Gcc => "gcc",
            Self::Cc => "cc",
        })
    }
}

/// The detected or configured linker acceleration for native artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkerKind {
    /// Default platform linker (e.g. GNU ld on Linux, ld64 on macOS, link.exe on Windows).
    #[default]
    Default,
    /// Fast modern ELF/Mach-O linker `mold`.
    Mold,
    /// LLVM modern linker `lld` (`ld.lld`).
    Lld,
}

impl LinkerKind {
    /// Command-line flag passed to compiler drivers to invoke this linker.
    pub const fn flag(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Mold => Some("-fuse-ld=mold"),
            Self::Lld => Some("-fuse-ld=lld"),
        }
    }
}

impl fmt::Display for LinkerKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => write!(formatter, "default"),
            Self::Mold => write!(formatter, "mold"),
            Self::Lld => write!(formatter, "lld"),
        }
    }
}

/// Identity captured when a compiler is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolIdentity {
    /// The executable selected by discovery or supplied by the caller.
    pub path: PathBuf,
    /// Canonicalized executable path when the path exists; otherwise the supplied path.
    ///
    /// Keeping both paths is useful for diagnostics (`path`) and for cache identity
    /// (`canonical_path`), where two symlinks to one compiler must not create separate
    /// toolchain identities.
    pub canonical_path: PathBuf,
    /// The inferred driver family.
    pub kind: CompilerKind,
    /// The first non-empty line returned by the driver's version probe.
    pub version: String,
    /// The target triple reported by `-dumpmachine`, or `"unknown"` when the driver does not
    /// support that probe.  An unavailable target probe is deliberately non-fatal so compiler
    /// discovery remains usable with unusual wrappers and older drivers; callers must retain the
    /// sentinel in cache keys rather than substituting the host target.
    pub target: String,
    /// The detected or configured linker acceleration.
    pub linker: LinkerKind,
}

impl ToolIdentity {
    /// Classify the target reported by this compiler.
    pub fn target_platform(&self) -> TargetPlatform {
        TargetPlatform::from_target(&self.target)
    }
}

impl ToolIdentity {
    /// Return a length-delimited, stable identity suitable for an artifact-cache key.
    ///
    /// The canonical path distinguishes SDK/toolchain installations that report the same
    /// version, while the compiler kind, version, and compiler-reported target distinguish their
    /// driver behavior and ABI.  Length delimiters avoid ambiguity when a version contains
    /// punctuation or whitespace.
    pub fn stable_key(&self) -> String {
        fn field(value: &str) -> String {
            format!("{}:{value}", value.len())
        }

        let mut parts = vec![
            field(&self.canonical_path.to_string_lossy()),
            field(&self.kind.to_string()),
            field(&self.version),
            field(&self.target),
        ];
        if self.linker != LinkerKind::Default {
            parts.push(field(&self.linker.to_string()));
        }
        parts.join("|")
    }
}

/// A command about to be executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Executable path.
    pub program: PathBuf,
    /// Arguments in process order.  They are never passed through a shell.
    pub args: Vec<OsString>,
    /// Optional working directory.
    pub current_dir: Option<PathBuf>,
    /// Exact environment overrides applied without clearing the inherited environment.
    pub environment: Vec<(OsString, OsString)>,
    /// Optional cross-process lock held for the complete child-process invocation.
    pub exclusive_lock: Option<PathBuf>,
}

impl CommandSpec {
    fn new(program: PathBuf, args: Vec<OsString>, current_dir: Option<PathBuf>) -> Self {
        Self {
            program,
            args,
            current_dir,
            environment: Vec::new(),
            exclusive_lock: None,
        }
    }

    fn with_environment_lock(
        mut self,
        environment: Vec<(OsString, OsString)>,
        exclusive_lock: Option<PathBuf>,
    ) -> Self {
        self.environment = environment;
        self.exclusive_lock = exclusive_lock;
        self
    }

    /// Render a diagnostic-friendly command line without invoking a shell.
    pub fn display_command(&self) -> String {
        let mut parts = Vec::with_capacity(self.args.len() + 1);
        parts.push(self.program.to_string_lossy().into_owned());
        parts.extend(self.args.iter().map(|argument| {
            let value = argument.to_string_lossy();
            if value.is_empty() || value.chars().any(char::is_whitespace) {
                format!("\"{value}\"")
            } else {
                value.into_owned()
            }
        }));
        parts.join(" ")
    }
}

/// The exit status exposed by a process runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessStatus {
    /// Whether the process reported success.
    pub success: bool,
    /// Conventional exit code, or `None` when the process was terminated by a signal.
    pub code: Option<i32>,
}

impl ProcessStatus {
    pub const fn success() -> Self {
        Self {
            success: true,
            code: Some(0),
        }
    }

    pub const fn failure(code: Option<i32>) -> Self {
        Self {
            success: false,
            code,
        }
    }
}

/// Captured result of a child process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub status: ProcessStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ProcessOutput {
    pub fn success(stdout: impl Into<Vec<u8>>, stderr: impl Into<Vec<u8>>) -> Self {
        Self {
            status: ProcessStatus::success(),
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    pub fn failure(
        code: Option<i32>,
        stdout: impl Into<Vec<u8>>,
        stderr: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            status: ProcessStatus::failure(code),
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }
}

/// Process execution abstraction used by [`Toolchain`].
pub trait ProcessRunner: Send + Sync {
    /// Execute one command, returning an I/O error only when the process could not be started or
    /// its output could not be collected.  A non-zero exit is represented by `ProcessOutput` so
    /// callers retain stdout and stderr in the structured [`ToolchainError::ProcessFailed`]. A
    /// custom runner must honor `environment` and `exclusive_lock` just like the system runner.
    fn run(&self, command: &CommandSpec) -> io::Result<ProcessOutput>;
}

/// The production child-process implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(&self, specification: &CommandSpec) -> io::Result<ProcessOutput> {
        let _lock = if let Some(path) = &specification.exclusive_lock {
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)?;
            file.lock()?;
            Some(file)
        } else {
            None
        };
        let mut command = Command::new(&specification.program);
        command
            .args(&specification.args)
            .envs(specification.environment.iter().cloned())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(directory) = &specification.current_dir {
            command.current_dir(directory);
        }

        let output = command.output()?;
        Ok(ProcessOutput {
            status: ProcessStatus {
                success: output.status.success(),
                code: output.status.code(),
            },
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

/// Native compilation profile from the design document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BuildProfile {
    /// Fast development objects and links.
    #[default]
    NativeDev,
    /// Debuggable objects with undefined-behavior sanitizer instrumentation.
    NativeCheck,
    /// Optimized release objects and links.
    NativeRelease,
    /// Release objects, plus `gprof` instrumentation.
    ///
    /// Release rather than debug on purpose: a profile of `-O0` code measures the wrong program,
    /// since the inlining that removes most prelude call overhead is exactly what `-O3` does. The
    /// `mcount` calls `-pg` inserts do perturb it -- a tiny hot function pays proportionally more
    /// than a large one -- so this answers "where does the time go", not "how fast is it".
    NativeProfile,
}

impl BuildProfile {
    fn compile_flags(self) -> &'static [&'static str] {
        match self {
            Self::NativeDev => &["-O0"],
            Self::NativeCheck => &["-O0", "-g", "-fsanitize=undefined"],
            Self::NativeRelease => &["-O3"],
            Self::NativeProfile => &["-O3", "-pg"],
        }
    }

    fn link_flags(self) -> &'static [&'static str] {
        match self {
            Self::NativeDev => &[],
            Self::NativeCheck => &["-fsanitize=undefined"],
            Self::NativeRelease => &["-O3"],
            Self::NativeProfile => &["-O3", "-pg"],
        }
    }
}

/// Options for compiling one emitted C translation unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileOptions {
    pub profile: BuildProfile,
    pub include_dirs: Vec<PathBuf>,
    pub extra_args: Vec<OsString>,
    pub current_dir: Option<PathBuf>,
    /// Emit position-independent code for objects that will be linked into a shared library.
    pub position_independent: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            profile: BuildProfile::NativeDev,
            include_dirs: Vec::new(),
            extra_args: Vec::new(),
            current_dir: None,
            position_independent: false,
        }
    }
}

/// Options for linking objects into one executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkOptions {
    pub profile: BuildProfile,
    /// Runtime or other library files passed as positional paths.
    pub runtime_libraries: Vec<PathBuf>,
    /// `-L` search paths.
    pub library_search_paths: Vec<PathBuf>,
    /// Library names passed as `-l<name>`.
    pub libraries: Vec<String>,
    pub extra_args: Vec<OsString>,
    pub current_dir: Option<PathBuf>,
}

impl Default for LinkOptions {
    fn default() -> Self {
        Self {
            profile: BuildProfile::NativeDev,
            runtime_libraries: Vec::new(),
            library_search_paths: Vec::new(),
            libraries: Vec::new(),
            extra_args: Vec::new(),
            current_dir: None,
        }
    }
}

/// A source/object pair in a multi-unit native build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileUnit {
    pub source: PathBuf,
    pub object: PathBuf,
}

impl CompileUnit {
    pub fn new(source: impl Into<PathBuf>, object: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            object: object.into(),
        }
    }
}

/// Combined options for [`Toolchain::build`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildOptions {
    pub compile: CompileOptions,
    pub link: LinkOptions,
}

/// The operation represented by a command report or error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuildStage {
    Probe,
    Compile,
    Link,
}

impl fmt::Display for BuildStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Probe => "compiler probe",
            Self::Compile => "C compilation",
            Self::Link => "native link",
        })
    }
}

/// Details retained for one completed compile or link invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactReport {
    pub stage: BuildStage,
    pub tool: ToolIdentity,
    pub output_path: PathBuf,
    pub output_size: u64,
    pub command: CommandSpec,
    pub status: ProcessStatus,
    pub elapsed: Duration,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ArtifactReport {
    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Report for compiling one or more objects and linking the final executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildReport {
    pub tool: ToolIdentity,
    pub objects: Vec<ArtifactReport>,
    pub executable: ArtifactReport,
    pub elapsed: Duration,
}

/// Details from a compiler version probe that returned a non-zero status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilerProbeFailure {
    pub path: PathBuf,
    pub command: CommandSpec,
    pub status: ProcessStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Details from a compile or link process that returned a non-zero status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessFailure {
    pub stage: BuildStage,
    pub command: CommandSpec,
    pub status: ProcessStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
}

/// Details retained when the compiler process could not be started or collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIoFailure {
    pub stage: BuildStage,
    pub command: CommandSpec,
    pub message: String,
}

/// Structured failure from discovery, process execution, or artifact inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolchainError {
    /// No executable candidate was found in the current `PATH`.
    NoCompiler { searched: Vec<String> },
    /// The chosen compiler could not answer its version probe.
    CompilerProbeFailed(Box<CompilerProbeFailure>),
    /// Kiln could not create its isolated Zig global cache.
    ZigCacheDirectory { path: PathBuf, message: String },
    /// The process could not be started or its output could not be collected.
    ProcessIo(Box<ProcessIoFailure>),
    /// The process ran but returned a non-zero status.
    ProcessFailed(Box<ProcessFailure>),
    /// A required input path did not exist or was not a regular file.
    InputMissing { stage: BuildStage, path: PathBuf },
    /// A successful command did not produce its promised output file.
    OutputMissing { stage: BuildStage, path: PathBuf },
    /// The requested artifact kind is not supported by this host platform.
    UnsupportedPlatform {
        operation: &'static str,
        platform: String,
    },
    /// Output metadata could not be read after a successful command.
    OutputMetadata {
        stage: BuildStage,
        path: PathBuf,
        message: String,
    },
}

impl ToolchainError {
    pub fn stage(&self) -> Option<BuildStage> {
        match self {
            Self::NoCompiler { .. }
            | Self::CompilerProbeFailed(_)
            | Self::ZigCacheDirectory { .. } => None,
            Self::ProcessIo(failure) => Some(failure.stage),
            Self::InputMissing { stage, .. }
            | Self::OutputMissing { stage, .. }
            | Self::OutputMetadata { stage, .. } => Some(*stage),
            Self::UnsupportedPlatform { .. } => None,
            Self::ProcessFailed(failure) => Some(failure.stage),
        }
    }

    fn command_error(stage: BuildStage, command: &CommandSpec, error: io::Error) -> Self {
        Self::ProcessIo(Box::new(ProcessIoFailure {
            stage,
            command: command.clone(),
            message: error.to_string(),
        }))
    }
}

impl fmt::Display for ToolchainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCompiler { searched } => write!(
                formatter,
                "no supported C compiler found (searched: {})",
                searched.join(", ")
            ),
            Self::CompilerProbeFailed(failure) => write!(
                formatter,
                "compiler `{}` failed version probe (status {:?}): {}",
                failure.path.display(),
                failure.status.code,
                String::from_utf8_lossy(&failure.stderr).trim()
            )
            .and_then(|_| write!(formatter, " [{}]", failure.command.display_command())),
            Self::ZigCacheDirectory { path, message } => write!(
                formatter,
                "creating isolated Zig cache {} failed: {message}",
                path.display()
            ),
            Self::ProcessIo(failure) => write!(
                formatter,
                "{} could not start: {} [{}]",
                failure.stage,
                failure.message,
                failure.command.display_command()
            ),
            Self::ProcessFailed(failure) => write!(
                formatter,
                "{} failed (status {:?}): {} [{}]",
                failure.stage,
                failure.status.code,
                String::from_utf8_lossy(&failure.stderr).trim(),
                failure.command.display_command()
            ),
            Self::InputMissing { stage, path } => {
                write!(
                    formatter,
                    "{stage} input does not exist: {}",
                    path.display()
                )
            }
            Self::OutputMissing { stage, path } => write!(
                formatter,
                "{stage} succeeded but did not produce output: {}",
                path.display()
            ),
            Self::UnsupportedPlatform {
                operation,
                platform,
            } => write!(
                formatter,
                "{operation} is unsupported on platform `{platform}`"
            ),
            Self::OutputMetadata {
                stage,
                path,
                message,
            } => write!(
                formatter,
                "{stage} output metadata failed for {}: {message}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ToolchainError {}

/// The `-target` `zig cc` needs to agree with the Rust side, or `None` to leave it alone.
///
/// `zig cc` defaults to musl on Linux. The private number provider is a Rust staticlib built for
/// the *host* triple, which on an ordinary Linux box is glibc, so the two disagree about libc and
/// the link fails with a target mismatch rather than a missing symbol. Zig can target glibc
/// perfectly well -- being a cross compiler is the point of it -- it simply has to be told.
///
/// The host's ABI is known without probing anything: this crate is compiled for the same target
/// the provider will be, so `cfg!` answers it. Zig spells a triple `arch-os-abi`, with no vendor
/// field, which is why this is built rather than reused from Rust's own triple.
///
/// `None` everywhere else on purpose. On macOS zig's default is already the host, and on a musl
/// host so is musl; forcing a triple there would replace a correct default with a guess.
fn zig_host_target() -> Option<String> {
    if cfg!(all(target_os = "linux", target_env = "gnu")) {
        Some(format!("{}-linux-gnu", std::env::consts::ARCH))
    } else {
        None
    }
}

/// `zig cc`, plus the target when this host needs one named.
fn zig_command_prefix() -> Vec<OsString> {
    let mut prefix = vec![OsString::from("cc")];
    if let Some(target) = zig_host_target() {
        prefix.push(OsString::from("-target"));
        prefix.push(OsString::from(target));
    }
    prefix
}

/// Auto-detect or select a linker strategy for a given compiler and target platform.
fn detect_linker(
    compiler: CompilerKind,
    platform: TargetPlatform,
    search_entries: &[PathBuf],
) -> LinkerKind {
    if compiler == CompilerKind::Zig || platform != TargetPlatform::Linux {
        return LinkerKind::Default;
    }

    if let Ok(env_val) = std::env::var("KILN_LINKER") {
        match env_val.trim().to_ascii_lowercase().as_str() {
            "mold" => return LinkerKind::Mold,
            "lld" | "ld.lld" => return LinkerKind::Lld,
            "default" | "system" | "ld" | "none" => return LinkerKind::Default,
            other => eprintln!(
                "kiln: unrecognized KILN_LINKER `{other}`; falling back to auto-detection"
            ),
        }
    }

    let is_in_path = |name: &str| -> bool {
        if !search_entries.is_empty() {
            for entry in search_entries {
                if is_executable_file(&entry.join(name)) {
                    return true;
                }
            }
            return false;
        }
        if let Some(path_var) = std::env::var_os("PATH") {
            for entry in std::env::split_paths(&path_var) {
                if is_executable_file(&entry.join(name)) {
                    return true;
                }
            }
        }
        false
    };

    if is_in_path("mold") || is_in_path("ld.mold") {
        LinkerKind::Mold
    } else if is_in_path("ld.lld") || is_in_path("lld") {
        LinkerKind::Lld
    } else {
        LinkerKind::Default
    }
}

/// A configured, versioned C compiler driver.
pub struct Toolchain {
    identity: ToolIdentity,
    /// `zig` needs the extra `cc` subcommand; ordinary drivers have no prefix.
    command_prefix: Vec<OsString>,
    zig_cache: Option<ZigCachePolicy>,
    runner: Arc<dyn ProcessRunner>,
}

#[derive(Debug, Clone)]
struct ZigCachePolicy {
    directory: OsString,
    lock_file: Option<PathBuf>,
}

impl fmt::Debug for Toolchain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Toolchain")
            .field("identity", &self.identity)
            .field("command_prefix", &self.command_prefix)
            .field("zig_cache", &self.zig_cache)
            .finish_non_exhaustive()
    }
}

impl Clone for Toolchain {
    fn clone(&self) -> Self {
        Self {
            identity: self.identity.clone(),
            command_prefix: self.command_prefix.clone(),
            zig_cache: self.zig_cache.clone(),
            runner: Arc::clone(&self.runner),
        }
    }
}

impl Toolchain {
    /// Find the first supported compiler in the process `PATH`.
    pub fn discover() -> Result<Self, ToolchainError> {
        Self::discover_with_runner(Arc::new(SystemProcessRunner))
    }

    /// Find a compiler in `PATH` using an injected runner.
    pub fn discover_with_runner(runner: Arc<dyn ProcessRunner>) -> Result<Self, ToolchainError> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let entries = std::env::split_paths(&path).collect::<Vec<_>>();
        let cc = std::env::var_os("CC");
        Self::discover_in_path_entries_with_cc(&entries, cc.as_deref(), runner)
    }

    /// Find a compiler in explicit path entries.  This is useful for hermetic callers and tests.
    pub fn discover_in_path_entries(
        entries: &[PathBuf],
        runner: Arc<dyn ProcessRunner>,
    ) -> Result<Self, ToolchainError> {
        Self::discover_in_path_entries_with_cc(entries, None, runner)
    }

    /// Find a compiler in explicit path entries, optionally honoring an explicit `CC` specification.
    pub fn discover_in_path_entries_with_cc(
        entries: &[PathBuf],
        cc: Option<&std::ffi::OsStr>,
        runner: Arc<dyn ProcessRunner>,
    ) -> Result<Self, ToolchainError> {
        if let Some(cc) = cc {
            let cc_trimmed = cc.to_str().map(str::trim).unwrap_or("");
            if !cc_trimmed.is_empty() {
                let mut parts = cc_trimmed.split_whitespace();
                let binary = parts.next().unwrap_or(cc_trimmed);
                let binary_path = Path::new(binary);

                if binary_path.is_absolute() || binary_path.components().count() > 1 {
                    if is_executable_file(binary_path) {
                        return Self::from_compiler_with_runner(binary_path, runner).map(
                            |toolchain| {
                                let platform = toolchain.identity().target_platform();
                                let linker =
                                    detect_linker(toolchain.identity().kind, platform, entries);
                                toolchain.with_linker(linker)
                            },
                        );
                    }
                } else {
                    for entry in entries {
                        let path = entry.join(binary);
                        if is_executable_file(&path)
                            && let Ok(toolchain) =
                                Self::from_compiler_with_runner(&path, Arc::clone(&runner))
                        {
                            let platform = toolchain.identity().target_platform();
                            let linker =
                                detect_linker(toolchain.identity().kind, platform, entries);
                            return Ok(toolchain.with_linker(linker));
                        }
                    }
                }
                return Err(ToolchainError::NoCompiler {
                    searched: vec![cc_trimmed.to_owned()],
                });
            }
        }
        let searched = DEFAULT_COMPILER_CANDIDATES
            .iter()
            .map(|candidate| (*candidate).to_owned())
            .collect::<Vec<_>>();
        for candidate in DEFAULT_COMPILER_CANDIDATES {
            for entry in entries {
                let path = entry.join(candidate);
                if is_executable_file(&path)
                    && let Ok(toolchain) =
                        Self::from_compiler_with_runner(&path, Arc::clone(&runner))
                {
                    let platform = toolchain.identity().target_platform();
                    let linker = detect_linker(toolchain.identity().kind, platform, entries);
                    return Ok(toolchain.with_linker(linker));
                }
            }
        }
        Err(ToolchainError::NoCompiler { searched })
    }

    /// Open a compiler using the production process runner.
    pub fn from_compiler(path: impl AsRef<Path>) -> Result<Self, ToolchainError> {
        Self::from_compiler_with_runner(path, Arc::new(SystemProcessRunner))
    }

    /// Open a compiler using a caller-provided process runner.
    pub fn from_compiler_with_runner(
        path: impl AsRef<Path>,
        runner: Arc<dyn ProcessRunner>,
    ) -> Result<Self, ToolchainError> {
        let path = path.as_ref().to_path_buf();
        let kind = infer_compiler_kind(&path);
        let command_prefix = if kind == CompilerKind::Zig {
            vec![OsString::from("version")] // only used by the probe below
        } else {
            Vec::new()
        };
        let probe = if kind == CompilerKind::Zig {
            CommandSpec::new(path.clone(), command_prefix.clone(), None)
        } else {
            CommandSpec::new(path.clone(), vec![OsString::from("--version")], None)
        };
        let output = runner
            .run(&probe)
            .map_err(|error| ToolchainError::command_error(BuildStage::Probe, &probe, error))?;
        if !output.status.success {
            return Err(ToolchainError::CompilerProbeFailed(Box::new(
                CompilerProbeFailure {
                    path,
                    command: probe,
                    status: output.status,
                    stdout: output.stdout,
                    stderr: output.stderr,
                },
            )));
        }
        let version = first_non_empty_line(&output.stdout)
            .or_else(|| first_non_empty_line(&output.stderr))
            .unwrap_or_else(|| "unknown".to_owned());
        let zig_cache = if kind == CompilerKind::Zig {
            Some(zig_cache_policy(&version)?)
        } else {
            None
        };
        // `-dumpmachine` is supported by GCC, Clang, and most `cc` wrappers.  Zig exposes the
        // same compiler query through its `cc` subcommand.  Some wrappers do not implement it,
        // so target discovery is best-effort: retaining `unknown` avoids making discovery fail
        // for otherwise usable toolchains while ensuring callers cannot accidentally substitute
        // the host target for the compiler's actual target.
        let target_probe = if kind == CompilerKind::Zig {
            CommandSpec::new(
                path.clone(),
                {
                    // Asked with the same `-target` it will be compiled with, or the probe would
                    // report musl and everything downstream would key on a triple never used.
                    let mut probe = zig_command_prefix();
                    probe.push(OsString::from("-dumpmachine"));
                    probe
                },
                None,
            )
            .with_environment_lock(
                zig_cache_environment(zig_cache.as_ref()),
                zig_cache
                    .as_ref()
                    .and_then(|policy| policy.lock_file.clone()),
            )
        } else {
            CommandSpec::new(path.clone(), vec![OsString::from("-dumpmachine")], None)
        };
        let target = runner
            .run(&target_probe)
            .ok()
            .filter(|target_output| target_output.status.success)
            .and_then(|target_output| {
                first_non_empty_line(&target_output.stdout)
                    .or_else(|| first_non_empty_line(&target_output.stderr))
            })
            .unwrap_or_else(|| "unknown".to_owned());
        let command_prefix = if kind == CompilerKind::Zig {
            zig_command_prefix()
        } else {
            Vec::new()
        };
        let canonical_path = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let platform = TargetPlatform::from_target(&target);
        let linker = detect_linker(kind, platform, &[]);
        Ok(Self {
            identity: ToolIdentity {
                canonical_path,
                path,
                kind,
                version,
                target,
                linker,
            },
            command_prefix,
            zig_cache,
            runner,
        })
    }

    pub fn identity(&self) -> &ToolIdentity {
        &self.identity
    }

    /// Return the detected or configured linker acceleration for this toolchain.
    pub fn linker(&self) -> LinkerKind {
        self.identity.linker
    }

    /// Override or configure the linker acceleration strategy for this toolchain.
    pub fn with_linker(mut self, linker: LinkerKind) -> Self {
        self.identity.linker = linker;
        self
    }

    /// Compile one emitted C translation unit to an object file.
    pub fn compile_c(
        &self,
        source: impl AsRef<Path>,
        object: impl AsRef<Path>,
        options: &CompileOptions,
    ) -> Result<ArtifactReport, ToolchainError> {
        let source = source.as_ref();
        let object = object.as_ref();
        require_input(BuildStage::Compile, source, options.current_dir.as_deref())?;

        let mut args = self.command_prefix.clone();
        args.push(OsString::from("-std=c11"));
        // A provider entry point the emitted C calls without a declaration is not a warning here.
        // C would give it an implicit `int f()` and link something whose ABI nobody checked; the
        // declarations are generated from the provider registry precisely so the compiler can name
        // the missing symbol, at the step where it went missing.
        args.push(OsString::from("-Werror=implicit-function-declaration"));
        // Report the source as though it sat in `.`, whatever directory it really occupies.
        //
        // Emitted C is written to a workspace named for the process that made it --
        // `kiln-native-<pid>-<n>-<nanos>` -- so every path the compiler *emits* would otherwise
        // carry that pid: `__FILE__`, the line table, and `DW_AT_comp_dir`. This covers those.
        // It does not cover paths the compiler composes into type names; the working directory
        // set further down is what covers those.
        if let Some(directory) = source
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            let mut map = OsString::from("-ffile-prefix-map=");
            map.push(directory.as_os_str());
            map.push("=.");
            args.push(map);
        }
        // Seed the compiler's internal name generation from the translation unit, not from where
        // it happens to be written.
        //
        // Absent `-frandom-seed`, gcc derives one from the *output* path, which is inside that
        // per-process workspace. The file name is stable across runs and distinct between the
        // units linked together, which is exactly what this seed has to be: it separates one
        // unit's local symbols from another's, and nothing about it should depend on the
        // directory.
        if let Some(name) = source.file_name() {
            let mut seed = OsString::from("-frandom-seed=");
            seed.push(name);
            args.push(seed);
        }
        args.push(OsString::from("-c"));
        for flag in options.profile.compile_flags() {
            args.push(OsString::from(flag));
        }
        if options.position_independent {
            args.push(OsString::from("-fPIC"));
        }
        for directory in &options.include_dirs {
            args.push(OsString::from("-I"));
            args.push(directory.as_os_str().to_owned());
        }
        args.extend(options.extra_args.iter().cloned());
        // Name the source by its file name and compile from its directory, rather than spelling
        // the absolute path on the command line.
        //
        // `-ffile-prefix-map` rewrites the paths a compiler *emits* -- `__FILE__`, the line table,
        // the compilation directory -- but not the ones it composes into type names. Clang names
        // an anonymous type after the file it was declared in, so an object compiled from
        // `/tmp/kiln-native-<pid>-<n>-<nanos>/prelude.c` carries
        // `union kiln_value::(anonymous at /tmp/kiln-native-1479071-.../prelude.c:1502:5)` in
        // `.debug_str`, and the pid in that string differs between processes. Two concurrent cold
        // builds of identical source therefore produced objects that differed in about twenty-two
        // thousand bytes of debug info, and the artifact cache -- which keys an object on its
        // source digest, and holds that one key means one payload -- refused the second with "same
        // cache key already contains a different payload".
        //
        // What the compiler cannot see, it cannot record: with the working directory set here,
        // the only path it is given is `prelude.c`.
        let (source_argument, working_directory) = match (source.file_name(), source.parent()) {
            (Some(name), Some(directory))
                if options.current_dir.is_none() && !directory.as_os_str().is_empty() =>
            {
                (name.to_owned(), Some(directory.to_path_buf()))
            }
            _ => (source.as_os_str().to_owned(), options.current_dir.clone()),
        };
        args.push(source_argument);
        args.push(OsString::from("-o"));
        args.push(object.as_os_str().to_owned());
        let command = self.command(args, working_directory.clone());
        self.run_artifact(
            BuildStage::Compile,
            command,
            object,
            working_directory.as_deref(),
        )
    }

    /// Link object files and optional runtime libraries into an executable.
    pub fn link_executable(
        &self,
        objects: &[PathBuf],
        executable: impl AsRef<Path>,
        options: &LinkOptions,
    ) -> Result<ArtifactReport, ToolchainError> {
        self.link_artifact(objects, executable.as_ref(), options, &[])
    }

    /// Link object files and optional runtime libraries into a shared library.
    pub fn link_shared_library(
        &self,
        objects: &[PathBuf],
        library: impl AsRef<Path>,
        options: &LinkOptions,
    ) -> Result<ArtifactReport, ToolchainError> {
        let shared_flags =
            shared_library_flags(self.identity.target_platform(), &self.identity.target)?;
        self.link_artifact(objects, library.as_ref(), options, shared_flags)
    }

    fn link_artifact(
        &self,
        objects: &[PathBuf],
        output: &Path,
        options: &LinkOptions,
        mode_flags: &[&str],
    ) -> Result<ArtifactReport, ToolchainError> {
        for object in objects {
            require_input(BuildStage::Link, object, options.current_dir.as_deref())?;
        }
        let mut args = self.command_prefix.clone();
        args.extend(mode_flags.iter().copied().map(OsString::from));
        if let Some(flag) = self.identity.linker.flag() {
            args.push(OsString::from(flag));
        }
        for flag in options.profile.link_flags() {
            args.push(OsString::from(flag));
        }
        args.extend(objects.iter().map(|object| object.as_os_str().to_owned()));
        for directory in &options.library_search_paths {
            args.push(OsString::from("-L"));
            args.push(directory.as_os_str().to_owned());
        }
        args.extend(
            options
                .runtime_libraries
                .iter()
                .map(|library| library.as_os_str().to_owned()),
        );
        for library in &options.libraries {
            args.push(OsString::from(format!("-l{library}")));
        }
        // The private number provider is a Rust staticlib, and Rust's panic machinery calls the
        // unwinder: `_Unwind_RaiseException`, `_Unwind_Backtrace`, `rust_eh_personality` and the
        // rest. `gcc` and `clang` link that implicitly, so nothing had to ask for it; `zig cc`
        // links only what it is told, and the failure is thirteen undefined `_Unwind_*` symbols
        // pointing into an archive that looks, from the message, like it is missing pieces.
        //
        // Zig ships the unwinder, so naming it is the whole fix. Last, after the archives that
        // reference it, because a static link resolves left to right.
        if self.identity.kind == CompilerKind::Zig {
            args.push(OsString::from("-lunwind"));
        }
        args.extend(options.extra_args.iter().cloned());
        args.push(OsString::from("-o"));
        args.push(output.as_os_str().to_owned());
        let command = self.command(args, options.current_dir.clone());
        self.run_artifact(
            BuildStage::Link,
            command,
            output,
            options.current_dir.as_deref(),
        )
    }

    /// Compile all units in order and link them in one process-owned build operation.
    pub fn build(
        &self,
        units: &[CompileUnit],
        executable: impl AsRef<Path>,
        options: &BuildOptions,
    ) -> Result<BuildReport, ToolchainError> {
        let started = Instant::now();
        let mut objects = Vec::with_capacity(units.len());
        for unit in units {
            objects.push(self.compile_c(&unit.source, &unit.object, &options.compile)?);
        }
        let object_paths = units
            .iter()
            .map(|unit| unit.object.clone())
            .collect::<Vec<_>>();
        let executable = self.link_executable(&object_paths, executable, &options.link)?;
        Ok(BuildReport {
            tool: self.identity.clone(),
            objects,
            executable,
            elapsed: started.elapsed(),
        })
    }

    fn command(&self, args: Vec<OsString>, current_dir: Option<PathBuf>) -> CommandSpec {
        CommandSpec::new(self.identity.path.clone(), args, current_dir).with_environment_lock(
            zig_cache_environment(self.zig_cache.as_ref()),
            self.zig_cache
                .as_ref()
                .and_then(|policy| policy.lock_file.clone()),
        )
    }

    fn run_artifact(
        &self,
        stage: BuildStage,
        command: CommandSpec,
        output_path: &Path,
        current_dir: Option<&Path>,
    ) -> Result<ArtifactReport, ToolchainError> {
        let started = Instant::now();
        let output = self
            .runner
            .run(&command)
            .map_err(|error| ToolchainError::command_error(stage, &command, error))?;
        let elapsed = started.elapsed();
        if !output.status.success {
            return Err(ToolchainError::ProcessFailed(Box::new(ProcessFailure {
                stage,
                command,
                status: output.status,
                stdout: output.stdout,
                stderr: output.stderr,
                elapsed,
            })));
        }
        let output_for_fs = resolve_path(output_path, current_dir);
        if !output_for_fs.is_file() {
            return Err(ToolchainError::OutputMissing {
                stage,
                path: output_path.to_path_buf(),
            });
        }
        let output_size = std::fs::metadata(&output_for_fs)
            .map(|metadata| metadata.len())
            .map_err(|error| ToolchainError::OutputMetadata {
                stage,
                path: output_path.to_path_buf(),
                message: error.to_string(),
            })?;
        Ok(ArtifactReport {
            stage,
            tool: self.identity.clone(),
            output_path: output_path.to_path_buf(),
            output_size,
            command,
            status: output.status,
            elapsed,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

fn zig_cache_policy(version: &str) -> Result<ZigCachePolicy, ToolchainError> {
    zig_cache_policy_with(
        version,
        std::env::var_os("ZIG_GLOBAL_CACHE_DIR"),
        &std::env::temp_dir(),
    )
}

fn zig_cache_policy_with(
    version: &str,
    inherited: Option<OsString>,
    temporary_root: &Path,
) -> Result<ZigCachePolicy, ToolchainError> {
    if let Some(directory) = inherited {
        return Ok(ZigCachePolicy {
            directory,
            lock_file: None,
        });
    }

    // Zig's ambient global cache is shared by every repository and process. Concurrent Kiln
    // worktrees have observed half-written runtime archives there, making an unrelated native test
    // fail until a developer manually replaces the cache. Keep one versioned Kiln cache instead
    // and serialize only Zig child processes across Kiln processes. A crash releases the OS lock;
    // the cache itself remains warm and Zig never races another Kiln writer.
    let version = version
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .take(80)
        .collect::<String>();
    let version = if version.is_empty() {
        "unknown"
    } else {
        version.as_str()
    };
    let directory = temporary_root
        .join("kiln-zig-global-cache-v1")
        .join(version);
    std::fs::create_dir_all(&directory).map_err(|error| ToolchainError::ZigCacheDirectory {
        path: directory.clone(),
        message: error.to_string(),
    })?;
    Ok(ZigCachePolicy {
        directory: directory.as_os_str().to_owned(),
        lock_file: Some(directory.join(".kiln-exclusive.lock")),
    })
}

fn zig_cache_environment(policy: Option<&ZigCachePolicy>) -> Vec<(OsString, OsString)> {
    policy
        .map(|policy| {
            vec![(
                OsString::from("ZIG_GLOBAL_CACHE_DIR"),
                policy.directory.clone(),
            )]
        })
        .unwrap_or_default()
}

fn require_input(
    stage: BuildStage,
    path: &Path,
    current_dir: Option<&Path>,
) -> Result<(), ToolchainError> {
    let path_for_fs = resolve_path(path, current_dir);
    match std::fs::metadata(path_for_fs) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) | Err(_) => Err(ToolchainError::InputMissing {
            stage,
            path: path.to_path_buf(),
        }),
    }
}

fn resolve_path(path: &Path, current_dir: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(current_dir) = current_dir {
        current_dir.join(path)
    } else {
        path.to_path_buf()
    }
}

fn shared_library_flags(
    platform: TargetPlatform,
    target: &str,
) -> Result<&'static [&'static str], ToolchainError> {
    platform
        .shared_link_flags()
        .ok_or_else(|| ToolchainError::UnsupportedPlatform {
            operation: "shared-library linking",
            platform: target.to_owned(),
        })
}

fn first_non_empty_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

fn infer_compiler_kind(path: &Path) -> CompilerKind {
    let name = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name == "zig" || name.starts_with("zig-") {
        CompilerKind::Zig
    } else if name.contains("clang") {
        CompilerKind::Clang
    } else if name.contains("gcc") || name.contains("g++") {
        CompilerKind::Gcc
    } else {
        CompilerKind::Cc
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::Mutex;

    #[derive(Debug)]
    struct FakeRunner {
        outputs: Mutex<VecDeque<ProcessOutput>>,
        commands: Mutex<Vec<CommandSpec>>,
        write_outputs: bool,
    }

    impl FakeRunner {
        fn new(outputs: impl IntoIterator<Item = ProcessOutput>, write_outputs: bool) -> Self {
            Self {
                outputs: Mutex::new(outputs.into_iter().collect()),
                commands: Mutex::new(Vec::new()),
                write_outputs,
            }
        }

        fn commands(&self) -> Vec<CommandSpec> {
            self.commands.lock().expect("commands lock").clone()
        }
    }

    impl ProcessRunner for FakeRunner {
        fn run(&self, command: &CommandSpec) -> io::Result<ProcessOutput> {
            self.commands
                .lock()
                .expect("commands lock")
                .push(command.clone());
            if self.write_outputs
                && let Some(index) = command.args.iter().position(|argument| argument == "-o")
                && let Some(path) = command.args.get(index + 1)
            {
                fs::write(path, b"artifact")
                    .map_err(|error| io::Error::other(error.to_string()))?;
            }
            self.outputs
                .lock()
                .expect("outputs lock")
                .pop_front()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::UnexpectedEof, "fake output exhausted")
                })
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "kiln-native-toolchain-{name}-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        fs::create_dir_all(&path).expect("temporary directory");
        path
    }

    #[test]
    fn fake_build_reports_flags_output_and_identity() {
        let directory = temp_dir("report");
        let source = directory.join("main.c");
        let object = directory.join("main.o");
        let executable = directory.join("app");
        fs::write(&source, "int main(void) { return 0; }\n").expect("source");
        let runner_impl = Arc::new(FakeRunner::new(
            [
                ProcessOutput::success("clang version 18.1\n", ""),
                ProcessOutput::success("arm64-apple-darwin24.0.0\n", ""),
                ProcessOutput::success("compile ok\n", "compile note\n"),
                ProcessOutput::success("link ok\n", ""),
            ],
            true,
        ));
        let runner: Arc<dyn ProcessRunner> = runner_impl.clone();
        let toolchain =
            Toolchain::from_compiler_with_runner("/opt/clang", runner).expect("compiler probe");
        let report = toolchain
            .build(
                &[CompileUnit::new(&source, &object)],
                &executable,
                &BuildOptions::default(),
            )
            .expect("build");

        assert_eq!(report.tool.kind, CompilerKind::Clang);
        assert_eq!(report.tool.version, "clang version 18.1");
        assert_eq!(report.tool.target, "arm64-apple-darwin24.0.0");
        assert_eq!(report.tool.canonical_path, PathBuf::from("/opt/clang"));
        assert!(
            report
                .tool
                .stable_key()
                .contains("arm64-apple-darwin24.0.0")
        );
        assert_eq!(report.objects[0].output_size, 8);
        assert_eq!(report.objects[0].stdout_lossy(), "compile ok\n");
        assert_eq!(report.objects[0].stderr_lossy(), "compile note\n");
        assert_eq!(report.executable.output_size, 8);
        assert!(report.elapsed >= report.executable.elapsed);
        let commands = runner_impl.commands();
        assert_eq!(commands.len(), 4);
        assert_eq!(commands[2].args[0], "-std=c11");
        assert!(commands[2].args.iter().any(|argument| argument == "-c"));
        assert!(commands[2].args.iter().any(|argument| argument == "-O0"));
        assert!(
            commands[3]
                .args
                .iter()
                .any(|argument| argument == object.as_os_str())
        );
        assert!(commands[3].args.iter().any(|argument| argument == "-o"));

        fs::remove_dir_all(directory).expect("temporary directory cleanup");
    }

    #[test]
    fn target_platform_classifies_reported_triples_and_policies() {
        let cases = [
            (
                "x86_64-unknown-linux-gnu",
                TargetPlatform::Linux,
                Some("so"),
                Some("program.so"),
                Some("m"),
                Some(&["-shared"][..]),
            ),
            (
                "aarch64-apple-darwin24.0.0",
                TargetPlatform::Macos,
                Some("dylib"),
                Some("program.dylib"),
                Some("m"),
                Some(&["-dynamiclib"][..]),
            ),
            (
                "aarch64-macos",
                TargetPlatform::Macos,
                Some("dylib"),
                Some("program.dylib"),
                Some("m"),
                Some(&["-dynamiclib"][..]),
            ),
            (
                "x86_64-pc-windows-msvc",
                TargetPlatform::Windows,
                Some("dll"),
                Some("program.dll"),
                None,
                None,
            ),
            ("", TargetPlatform::Unknown, None, None, None, None),
            (
                "x86_64-unknown-haiku",
                TargetPlatform::Other,
                None,
                None,
                None,
                None,
            ),
        ];

        for (target, expected, suffix, name, math, shared_flags) in cases {
            let platform = TargetPlatform::from_target(target);
            assert_eq!(platform, expected, "target `{target}`");
            assert_eq!(
                platform.shared_library_suffix(),
                suffix,
                "target `{target}`"
            );
            assert_eq!(platform.shared_library_name(), name, "target `{target}`");
            assert_eq!(platform.system_math_library(), math, "target `{target}`");
            assert_eq!(
                platform.shared_link_flags(),
                shared_flags,
                "target `{target}`"
            );
        }
        assert_eq!(TargetPlatform::Windows.executable_suffix(), ".exe");
        assert_eq!(TargetPlatform::Linux.executable_suffix(), "");
        assert_eq!(TargetPlatform::Unknown.executable_suffix(), "");
    }

    #[test]
    fn unknown_target_never_uses_host_shared_link_mode() {
        let directory = temp_dir("unknown-target");
        let object = directory.join("module.o");
        fs::write(&object, b"object").expect("object");
        let runner_impl = Arc::new(FakeRunner::new(
            [
                ProcessOutput::success("cc wrapper 1.0\n", ""),
                ProcessOutput::failure(Some(1), "", "no target probe"),
            ],
            false,
        ));
        let runner: Arc<dyn ProcessRunner> = runner_impl.clone();
        let toolchain =
            Toolchain::from_compiler_with_runner("/opt/custom-cc", runner).expect("compiler probe");
        assert_eq!(
            toolchain.identity().target_platform(),
            TargetPlatform::Unknown
        );
        let error = toolchain
            .link_shared_library(
                &[object],
                directory.join("module.so"),
                &LinkOptions::default(),
            )
            .expect_err("unknown target must not select host shared-link flags");
        assert!(matches!(
            error,
            ToolchainError::UnsupportedPlatform { ref platform, .. } if platform == "unknown"
        ));
        assert_eq!(
            runner_impl.commands().len(),
            2,
            "linker must not be invoked"
        );
        fs::remove_dir_all(directory).expect("temporary directory cleanup");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn fake_shared_link_reports_pic_and_platform_flags() {
        let directory = temp_dir("shared");
        let source = directory.join("module.c");
        let object = directory.join("module.o");
        let library = directory.join("libkiln-module.dylib");
        fs::write(&source, "int kiln_value(void) { return 7; }\n").expect("source");
        let runner_impl = Arc::new(FakeRunner::new(
            [
                ProcessOutput::success("clang version 18.1\n", ""),
                ProcessOutput::success("arm64-apple-darwin24.0.0\n", ""),
                ProcessOutput::success("compile ok\n", "compile note\n"),
                ProcessOutput::success("link ok\n", "link note\n"),
            ],
            true,
        ));
        let runner: Arc<dyn ProcessRunner> = runner_impl.clone();
        let toolchain =
            Toolchain::from_compiler_with_runner("/opt/clang", runner).expect("compiler probe");
        let compile_report = toolchain
            .compile_c(
                &source,
                &object,
                &CompileOptions {
                    position_independent: true,
                    ..CompileOptions::default()
                },
            )
            .expect("PIC compile");
        let link_report = toolchain
            .link_shared_library(
                std::slice::from_ref(&object),
                &library,
                &LinkOptions::default(),
            )
            .expect("shared link");

        assert_eq!(compile_report.stage, BuildStage::Compile);
        assert_eq!(compile_report.stdout_lossy(), "compile ok\n");
        assert_eq!(compile_report.stderr_lossy(), "compile note\n");
        assert_eq!(link_report.stage, BuildStage::Link);
        assert_eq!(link_report.stdout_lossy(), "link ok\n");
        assert_eq!(link_report.stderr_lossy(), "link note\n");
        let commands = runner_impl.commands();
        assert_eq!(commands.len(), 4);
        assert!(commands[2].args.iter().any(|argument| argument == "-fPIC"));
        assert!(commands[3].args.iter().any(|argument| {
            shared_library_flags(
                toolchain.identity().target_platform(),
                toolchain.identity().target.as_str(),
            )
            .expect("supported platform")
            .iter()
            .any(|flag| argument == flag)
        }));
        assert!(
            commands[3]
                .args
                .iter()
                .any(|argument| argument == object.as_os_str())
        );
        assert_eq!(link_report.output_path, library);
        assert_eq!(link_report.output_size, 8);

        fs::remove_dir_all(directory).expect("temporary directory cleanup");
    }

    #[test]
    fn zig_driver_inserts_cc_subcommand() {
        let runner: Arc<dyn ProcessRunner> = Arc::new(FakeRunner::new(
            [
                ProcessOutput::success("0.14.0\n", ""),
                ProcessOutput::success("aarch64-macos\n", ""),
            ],
            false,
        ));
        let toolchain =
            Toolchain::from_compiler_with_runner("/opt/zig", runner).expect("compiler probe");
        assert_eq!(toolchain.identity().kind, CompilerKind::Zig);
        assert_eq!(toolchain.identity().version, "0.14.0");
        assert_eq!(toolchain.identity().target, "aarch64-macos");

        // The version probe is followed by `zig cc [-target <host>] -dumpmachine`; the latter is
        // the target query understood by Zig's C driver rather than a shell-expanded command.
        // The `-target` is present exactly where `zig_host_target` says the default is wrong, and
        // the probe has to carry it or it would report a triple nothing is built with.
        let runner_impl = Arc::new(FakeRunner::new(
            [
                ProcessOutput::success("0.14.0\n", ""),
                ProcessOutput::success("aarch64-macos\n", ""),
            ],
            false,
        ));
        let runner: Arc<dyn ProcessRunner> = runner_impl.clone();
        let _ = Toolchain::from_compiler_with_runner("/opt/zig", runner).expect("probe");
        let commands = runner_impl.commands();
        let mut expected_probe = zig_command_prefix();
        expected_probe.push(OsString::from("-dumpmachine"));
        assert_eq!(commands[1].args, expected_probe);
        assert_eq!(commands[1].args.first(), Some(&OsString::from("cc")));
        // On a glibc Linux host the default really is wrong, so the target has to be named.
        if cfg!(all(target_os = "linux", target_env = "gnu")) {
            assert!(
                commands[1].args.contains(&OsString::from("-target")),
                "a glibc host must ask zig for glibc: {:?}",
                commands[1].args
            );
        }
        assert_eq!(
            commands[1]
                .environment
                .iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            [&OsString::from("ZIG_GLOBAL_CACHE_DIR")]
        );
    }

    #[test]
    fn managed_zig_cache_is_versioned_locked_and_explicit_policy_is_preserved() {
        let temporary_root = temp_dir("managed-zig-cache");
        let managed = zig_cache_policy_with("0.14.0 dev build", None, &temporary_root)
            .expect("create managed Zig cache");
        let managed_path = PathBuf::from(&managed.directory);
        assert_eq!(
            managed_path,
            temporary_root
                .join("kiln-zig-global-cache-v1")
                .join("0.14.0-dev-build")
        );
        assert!(managed_path.is_dir());
        assert_eq!(
            managed.lock_file,
            Some(managed_path.join(".kiln-exclusive.lock"))
        );
        assert_eq!(
            zig_cache_environment(Some(&managed)),
            [(
                OsString::from("ZIG_GLOBAL_CACHE_DIR"),
                managed.directory.clone()
            )]
        );

        let explicit = zig_cache_policy_with(
            "0.14.0",
            Some(OsString::from("/chosen/zig-cache")),
            &temporary_root,
        )
        .expect("preserve explicit Zig cache");
        assert_eq!(explicit.directory, OsString::from("/chosen/zig-cache"));
        assert_eq!(explicit.lock_file, None);

        fs::remove_dir_all(temporary_root).expect("temporary directory cleanup");
    }

    #[test]
    fn every_zig_cc_stage_uses_one_managed_cache_and_lock() {
        let directory = temp_dir("zig-stage-environment");
        let source = directory.join("unit.c");
        let object = directory.join("unit.o");
        let executable = directory.join("unit");
        fs::write(&source, "int main(void) { return 0; }").expect("source fixture");
        let runner_impl = Arc::new(FakeRunner::new(
            [
                ProcessOutput::success("0.14.0\n", ""),
                ProcessOutput::success("aarch64-macos\n", ""),
                ProcessOutput::success("", ""),
                ProcessOutput::success("", ""),
            ],
            true,
        ));
        let runner: Arc<dyn ProcessRunner> = runner_impl.clone();
        let toolchain =
            Toolchain::from_compiler_with_runner("/opt/zig", runner).expect("compiler probe");
        toolchain
            .compile_c(&source, &object, &CompileOptions::default())
            .expect("Zig compile");
        toolchain
            .link_executable(&[object], &executable, &LinkOptions::default())
            .expect("Zig link");

        let commands = runner_impl.commands();
        let target = &commands[1];
        for command in &commands[2..] {
            assert_eq!(command.environment, target.environment);
            assert_eq!(command.exclusive_lock, target.exclusive_lock);
        }

        fs::remove_dir_all(directory).expect("temporary directory cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn system_runner_serializes_commands_that_share_an_exclusive_lock() {
        use std::thread;

        let directory = temp_dir("exclusive-runner-lock");
        let script = directory.join("hold-marker.sh");
        let marker = directory.join("marker");
        let ready = directory.join("ready");
        let release = directory.join("release");
        let lock = directory.join("compiler.lock");
        fs::write(
            &script,
            "if ! mkdir \"$1\"; then\n  exit 41\nfi\n: > \"$2\"\nwhile [ ! -e \"$3\" ]; do\n  sleep 0.01\ndone\nrmdir \"$1\"\n",
        )
        .expect("write lock fixture");
        let specification = CommandSpec::new(
            PathBuf::from("/bin/sh"),
            vec![
                script.as_os_str().to_owned(),
                marker.as_os_str().to_owned(),
                ready.as_os_str().to_owned(),
                release.as_os_str().to_owned(),
            ],
            None,
        )
        .with_environment_lock(Vec::new(), Some(lock));

        let first_specification = specification.clone();
        let first = thread::spawn(move || SystemProcessRunner.run(&first_specification));
        for _ in 0..100 {
            if ready.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(ready.exists(), "first command acquired the marker");

        let second = thread::spawn(move || SystemProcessRunner.run(&specification));
        thread::sleep(Duration::from_millis(25));
        fs::write(&release, "release").expect("release first command");

        assert!(
            first
                .join()
                .expect("first runner thread")
                .unwrap()
                .status
                .success
        );
        assert!(
            second
                .join()
                .expect("second runner thread")
                .unwrap()
                .status
                .success,
            "the second command runs only after the first releases the marker"
        );

        fs::remove_dir_all(directory).expect("temporary directory cleanup");
    }

    #[test]
    fn unavailable_target_probe_keeps_discovery_usable() {
        let runner_impl = Arc::new(FakeRunner::new(
            [
                ProcessOutput::success("cc (wrapper) 1.0\n", ""),
                ProcessOutput::failure(Some(1), "", "unknown option: -dumpmachine"),
            ],
            false,
        ));
        let runner: Arc<dyn ProcessRunner> = runner_impl.clone();
        let toolchain =
            Toolchain::from_compiler_with_runner("/opt/custom-cc", runner).expect("probe");
        assert_eq!(toolchain.identity().target, "unknown");
        assert_eq!(runner_impl.commands().len(), 2);
    }

    #[test]
    fn process_failure_keeps_captured_streams_and_command() {
        let directory = temp_dir("failure");
        let source = directory.join("bad.c");
        fs::write(&source, "not valid C").expect("source");
        let runner: Arc<dyn ProcessRunner> = Arc::new(FakeRunner::new(
            [
                ProcessOutput::success("cc version\n", ""),
                ProcessOutput::success("arm64-apple-darwin24.0.0\n", ""),
                ProcessOutput::failure(Some(2), "compiler output", "syntax error"),
            ],
            false,
        ));
        let toolchain =
            Toolchain::from_compiler_with_runner("/opt/cc", runner).expect("compiler probe");
        let error = toolchain
            .compile_c(&source, directory.join("bad.o"), &CompileOptions::default())
            .expect_err("compile should fail");
        match error {
            ToolchainError::ProcessFailed(failure) => {
                assert_eq!(failure.stage, BuildStage::Compile);
                assert_eq!(failure.status.code, Some(2));
                assert_eq!(failure.stdout, b"compiler output");
                assert_eq!(failure.stderr, b"syntax error");
            }
            other => panic!("unexpected error: {other:?}"),
        }
        fs::remove_dir_all(directory).expect("temporary directory cleanup");
    }

    #[test]
    fn missing_input_is_reported_before_process_start() {
        let runner: Arc<dyn ProcessRunner> = Arc::new(FakeRunner::new(
            [ProcessOutput::success("cc version\n", "")],
            false,
        ));
        let toolchain =
            Toolchain::from_compiler_with_runner("/opt/cc", runner).expect("compiler probe");
        let error = toolchain
            .compile_c(
                "/definitely/missing.c",
                "/tmp/missing.o",
                &CompileOptions::default(),
            )
            .expect_err("missing source");
        assert_eq!(
            error,
            ToolchainError::InputMissing {
                stage: BuildStage::Compile,
                path: PathBuf::from("/definitely/missing.c")
            }
        );
    }

    #[test]
    fn real_compiler_round_trip_or_explicit_skip() {
        let Ok(toolchain) = Toolchain::discover() else {
            eprintln!("skipping native toolchain round-trip: no supported C compiler found");
            return;
        };
        let directory = temp_dir("real");
        let source = directory.join("main.c");
        let object = directory.join("main.o");
        let executable = directory.join("kiln-native-smoke");
        fs::write(
            &source,
            "#include <stdio.h>\nint main(void) { puts(\"kiln\"); return 0; }\n",
        )
        .expect("source");
        let report = toolchain
            .build(
                &[CompileUnit::new(&source, &object)],
                &executable,
                &BuildOptions::default(),
            )
            .expect("native compiler round trip");
        assert!(report.objects[0].output_size > 0);
        assert!(report.executable.output_size > 0);
        fs::remove_dir_all(directory).expect("temporary directory cleanup");
    }

    #[test]
    fn real_shared_library_round_trip_or_explicit_skip() {
        let Ok(toolchain) = Toolchain::discover() else {
            eprintln!("skipping shared-library round-trip: no supported C compiler found");
            return;
        };
        if !cfg!(any(target_os = "macos", target_os = "linux")) {
            eprintln!("skipping shared-library round-trip: unsupported host platform");
            return;
        }
        let directory = temp_dir("real-shared");
        let source = directory.join("module.c");
        let object = directory.join("module.o");
        let library = if cfg!(target_os = "macos") {
            directory.join("libkiln-module.dylib")
        } else {
            directory.join("libkiln-module.so")
        };
        fs::write(&source, "int kiln_value(void) { return 7; }\n").expect("source");
        toolchain
            .compile_c(
                &source,
                &object,
                &CompileOptions {
                    position_independent: true,
                    ..CompileOptions::default()
                },
            )
            .expect("PIC compile");
        let report = toolchain
            .link_shared_library(&[object], &library, &LinkOptions::default())
            .expect("shared link");
        assert!(report.output_size > 0);
        fs::remove_dir_all(directory).expect("temporary directory cleanup");
    }

    #[test]
    fn discover_with_cc_selects_explicit_compiler() {
        let directory = temp_dir("cc-discovery");
        let gcc_path = directory.join("gcc");
        let clang_path = directory.join("clang");
        fs::write(&gcc_path, "").expect("write fake gcc");
        fs::write(&clang_path, "").expect("write fake clang");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&gcc_path, fs::Permissions::from_mode(0o755)).expect("chmod gcc");
            fs::set_permissions(&clang_path, fs::Permissions::from_mode(0o755))
                .expect("chmod clang");
        }

        let runner_impl = Arc::new(FakeRunner::new(
            [
                ProcessOutput::success("gcc (Ubuntu 13.2.0) 13.2.0\n", ""),
                ProcessOutput::success("x86_64-linux-gnu\n", ""),
            ],
            false,
        ));
        let runner: Arc<dyn ProcessRunner> = runner_impl.clone();

        let toolchain = Toolchain::discover_in_path_entries_with_cc(
            std::slice::from_ref(&directory),
            Some(std::ffi::OsStr::new("gcc")),
            runner,
        )
        .expect("discover explicit gcc");

        assert_eq!(toolchain.identity().kind, CompilerKind::Gcc);
        assert_eq!(toolchain.identity().path, gcc_path);

        let runner_empty: Arc<dyn ProcessRunner> = Arc::new(FakeRunner::new([], false));
        let error = Toolchain::discover_in_path_entries_with_cc(
            std::slice::from_ref(&directory),
            Some(std::ffi::OsStr::new("nonexistent-cc")),
            runner_empty,
        )
        .expect_err("nonexistent CC must fail");

        match error {
            ToolchainError::NoCompiler { searched } => {
                assert_eq!(searched, vec!["nonexistent-cc".to_owned()]);
            }
            other => panic!("unexpected error: {other:?}"),
        }

        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn linker_flags_and_stable_key_reflect_acceleration() {
        assert_eq!(LinkerKind::Default.flag(), None);
        assert_eq!(LinkerKind::Mold.flag(), Some("-fuse-ld=mold"));
        assert_eq!(LinkerKind::Lld.flag(), Some("-fuse-ld=lld"));

        let identity_default = ToolIdentity {
            path: PathBuf::from("clang"),
            canonical_path: PathBuf::from("/usr/bin/clang"),
            kind: CompilerKind::Clang,
            version: "18.1.3".to_owned(),
            target: "x86_64-linux-gnu".to_owned(),
            linker: LinkerKind::Default,
        };
        let mut identity_mold = identity_default.clone();
        identity_mold.linker = LinkerKind::Mold;

        let mut identity_lld = identity_default.clone();
        identity_lld.linker = LinkerKind::Lld;

        assert_ne!(identity_default.stable_key(), identity_mold.stable_key());
        assert_ne!(identity_mold.stable_key(), identity_lld.stable_key());
        assert!(identity_mold.stable_key().contains("mold"));
        assert!(identity_lld.stable_key().contains("lld"));
    }

    #[test]
    fn linker_auto_detection_prefers_mold_then_lld() {
        let directory = temp_dir("linker-detection");
        let mold_path = directory.join("mold");
        let lld_path = directory.join("ld.lld");
        fs::write(&mold_path, "").expect("write fake mold");
        fs::write(&lld_path, "").expect("write fake lld");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&mold_path, fs::Permissions::from_mode(0o755)).expect("chmod mold");
            fs::set_permissions(&lld_path, fs::Permissions::from_mode(0o755)).expect("chmod lld");
        }

        // Linux + Clang with both mold and lld present selects mold
        let detected = detect_linker(
            CompilerKind::Clang,
            TargetPlatform::Linux,
            std::slice::from_ref(&directory),
        );
        assert_eq!(detected, LinkerKind::Mold);

        // Without mold, selects lld
        fs::remove_file(&mold_path).expect("remove mold");
        let detected_lld = detect_linker(
            CompilerKind::Clang,
            TargetPlatform::Linux,
            std::slice::from_ref(&directory),
        );
        assert_eq!(detected_lld, LinkerKind::Lld);

        // Zig stays Default regardless of linkers in path
        assert_eq!(
            detect_linker(
                CompilerKind::Zig,
                TargetPlatform::Linux,
                std::slice::from_ref(&directory),
            ),
            LinkerKind::Default
        );

        // Non-Linux stays Default
        assert_eq!(
            detect_linker(
                CompilerKind::Clang,
                TargetPlatform::Macos,
                std::slice::from_ref(&directory),
            ),
            LinkerKind::Default
        );

        fs::remove_dir_all(directory).expect("cleanup");
    }
}
