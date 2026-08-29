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

        [
            field(&self.canonical_path.to_string_lossy()),
            field(&self.kind.to_string()),
            field(&self.version),
            field(&self.target),
        ]
        .join("|")
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
}

impl CommandSpec {
    fn new(program: PathBuf, args: Vec<OsString>, current_dir: Option<PathBuf>) -> Self {
        Self {
            program,
            args,
            current_dir,
        }
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
    /// callers retain stdout and stderr in the structured [`ToolchainError::ProcessFailed`].
    fn run(&self, command: &CommandSpec) -> io::Result<ProcessOutput>;
}

/// The production child-process implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(&self, specification: &CommandSpec) -> io::Result<ProcessOutput> {
        let mut command = Command::new(&specification.program);
        command
            .args(&specification.args)
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
}

impl BuildProfile {
    fn compile_flags(self) -> &'static [&'static str] {
        match self {
            Self::NativeDev => &["-O0"],
            Self::NativeCheck => &["-O0", "-g", "-fsanitize=undefined"],
            Self::NativeRelease => &["-O3"],
        }
    }

    fn link_flags(self) -> &'static [&'static str] {
        match self {
            Self::NativeDev => &[],
            Self::NativeCheck => &["-fsanitize=undefined"],
            Self::NativeRelease => &["-O3"],
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

/// Structured failure from discovery, process execution, or artifact inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolchainError {
    /// No executable candidate was found in the current `PATH`.
    NoCompiler { searched: Vec<String> },
    /// The chosen compiler could not answer its version probe.
    CompilerProbeFailed(Box<CompilerProbeFailure>),
    /// The process could not be started or its output could not be collected.
    ProcessIo {
        stage: BuildStage,
        command: CommandSpec,
        message: String,
    },
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
            Self::NoCompiler { .. } | Self::CompilerProbeFailed(_) => None,
            Self::ProcessIo { stage, .. }
            | Self::InputMissing { stage, .. }
            | Self::OutputMissing { stage, .. }
            | Self::OutputMetadata { stage, .. } => Some(*stage),
            Self::UnsupportedPlatform { .. } => None,
            Self::ProcessFailed(failure) => Some(failure.stage),
        }
    }

    fn command_error(stage: BuildStage, command: &CommandSpec, error: io::Error) -> Self {
        Self::ProcessIo {
            stage,
            command: command.clone(),
            message: error.to_string(),
        }
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
            Self::ProcessIo {
                stage,
                command,
                message,
            } => write!(
                formatter,
                "{stage} could not start: {message} [{}]",
                command.display_command()
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

/// A configured, versioned C compiler driver.
pub struct Toolchain {
    identity: ToolIdentity,
    /// `zig` needs the extra `cc` subcommand; ordinary drivers have no prefix.
    command_prefix: Vec<OsString>,
    runner: Arc<dyn ProcessRunner>,
}

impl fmt::Debug for Toolchain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Toolchain")
            .field("identity", &self.identity)
            .field("command_prefix", &self.command_prefix)
            .finish_non_exhaustive()
    }
}

impl Clone for Toolchain {
    fn clone(&self) -> Self {
        Self {
            identity: self.identity.clone(),
            command_prefix: self.command_prefix.clone(),
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
        Self::discover_in_path_entries(&entries, runner)
    }

    /// Find a compiler in explicit path entries.  This is useful for hermetic callers and tests.
    pub fn discover_in_path_entries(
        entries: &[PathBuf],
        runner: Arc<dyn ProcessRunner>,
    ) -> Result<Self, ToolchainError> {
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
                    return Ok(toolchain);
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
        // `-dumpmachine` is supported by GCC, Clang, and most `cc` wrappers.  Zig exposes the
        // same compiler query through its `cc` subcommand.  Some wrappers do not implement it,
        // so target discovery is best-effort: retaining `unknown` avoids making discovery fail
        // for otherwise usable toolchains while ensuring callers cannot accidentally substitute
        // the host target for the compiler's actual target.
        let target_probe = if kind == CompilerKind::Zig {
            CommandSpec::new(
                path.clone(),
                vec![OsString::from("cc"), OsString::from("-dumpmachine")],
                None,
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
            vec![OsString::from("cc")]
        } else {
            Vec::new()
        };
        let canonical_path = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        Ok(Self {
            identity: ToolIdentity {
                canonical_path,
                path,
                kind,
                version,
                target,
            },
            command_prefix,
            runner,
        })
    }

    pub fn identity(&self) -> &ToolIdentity {
        &self.identity
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
        args.push(source.as_os_str().to_owned());
        args.push(OsString::from("-o"));
        args.push(object.as_os_str().to_owned());
        let command = self.command(args, options.current_dir.clone());
        self.run_artifact(
            BuildStage::Compile,
            command,
            object,
            options.current_dir.as_deref(),
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
        let shared_flags = shared_library_flags(&self.identity.target)?;
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
        CommandSpec::new(self.identity.path.clone(), args, current_dir)
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

fn shared_library_flags(target: &str) -> Result<&'static [&'static str], ToolchainError> {
    if target.contains("darwin") {
        Ok(&["-dynamiclib"])
    } else if target.contains("linux") {
        Ok(&["-shared"])
    } else if target == "unknown" && cfg!(target_os = "macos") {
        Ok(&["-dynamiclib"])
    } else if target == "unknown" && cfg!(target_os = "linux") {
        Ok(&["-shared"])
    } else {
        Err(ToolchainError::UnsupportedPlatform {
            operation: "shared-library linking",
            platform: target.to_owned(),
        })
    }
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
            shared_library_flags(toolchain.identity().target.as_str())
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

        // The version probe is followed by `zig cc -dumpmachine`; the latter is the target
        // query understood by Zig's C driver rather than a shell-expanded command.
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
        assert_eq!(
            commands[1].args,
            [OsString::from("cc"), OsString::from("-dumpmachine")]
        );
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
}
