//! Cached orchestration for Kiln's C native backend.
//!
//! The backend, runtime, cache, and platform toolchain deliberately live in separate crates.
//! This crate is the narrow coordinator that gives those pieces an ordered build graph:
//! generated module C, application object, reusable runtime object, launcher object, executable.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use kiln_artifact_cache::{
    ArtifactCache, ArtifactKey, ArtifactKind, Backend, BuildProfile as CacheProfile, CacheError,
    Digest, Lookup,
};
use kiln_backend_c::{C_ABI_VERSION, EmitError, emit_c};
use kiln_host_abi::HOST_ABI_VERSION;
use kiln_ir::Program;
use kiln_native_runtime::{
    NATIVE_RUNTIME_VERSION, RuntimeSourceError, launcher_source, runtime_source,
};
use kiln_native_toolchain::{
    BuildProfile as ToolchainProfile, CompileOptions, LinkOptions, ToolIdentity, Toolchain,
    ToolchainError,
};

static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);
static MATERIALIZE_COUNTER: AtomicU64 = AtomicU64::new(0);
static DISCOVERED_TOOLCHAIN: OnceLock<Result<Toolchain, ToolchainError>> = OnceLock::new();

/// Native optimization and checking policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum NativeProfile {
    #[default]
    Dev,
    Check,
    Release,
}

impl NativeProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "native-dev",
            Self::Check => "native-check",
            Self::Release => "native-release",
        }
    }

    const fn cache(self) -> CacheProfile {
        match self {
            Self::Dev => CacheProfile::Debug,
            Self::Check => CacheProfile::Check,
            Self::Release => CacheProfile::Release,
        }
    }

    const fn toolchain(self) -> ToolchainProfile {
        match self {
            Self::Dev => ToolchainProfile::NativeDev,
            Self::Check => ToolchainProfile::NativeCheck,
            Self::Release => ToolchainProfile::NativeRelease,
        }
    }
}

/// Inputs controlled by a native-build caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeBuildOptions {
    pub output: PathBuf,
    pub profile: NativeProfile,
}

/// Which artifact stages were reused without invoking the platform compiler or linker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheHits {
    pub generated_c: bool,
    pub application_object: bool,
    pub runtime_object: bool,
    pub launcher_object: bool,
    pub executable: bool,
}

/// Byte sizes of the material build products.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArtifactSizes {
    pub generated_c: u64,
    pub application_object: u64,
    pub runtime_object: u64,
    pub launcher_object: u64,
    pub executable: u64,
}

/// Time spent in the native pipeline. Cache hits contribute no compile/link time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuildTimings {
    pub emit: Duration,
    pub compile: Duration,
    pub link: Duration,
    pub total: Duration,
}

/// Observable result of one cached native build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeBuildReport {
    pub output: PathBuf,
    pub module_symbol: String,
    pub tool: ToolIdentity,
    pub cache_hits: CacheHits,
    pub sizes: ArtifactSizes,
    pub timings: BuildTimings,
}

/// Captured result of launching a freshly built or cached executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRunReport {
    pub build: NativeBuildReport,
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl NativeRunReport {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Failure from emission, caching, compilation, materialization, or launch.
#[derive(Debug)]
pub enum NativeError {
    Emit(EmitError),
    Runtime(RuntimeSourceError),
    Cache(CacheError),
    Toolchain(ToolchainError),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Launch {
        executable: PathBuf,
        source: io::Error,
    },
    WorkerPanic {
        stage: &'static str,
    },
}

impl fmt::Display for NativeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Emit(error) => write!(formatter, "native emission failed: {error}"),
            Self::Runtime(error) => write!(formatter, "native runtime source failed: {error}"),
            Self::Cache(error) => write!(formatter, "native cache failed: {error}"),
            Self::Toolchain(error) => write!(formatter, "native toolchain failed: {error}"),
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "could not {operation} {}: {source}",
                path.display()
            ),
            Self::Launch { executable, source } => {
                write!(
                    formatter,
                    "could not launch {}: {source}",
                    executable.display()
                )
            }
            Self::WorkerPanic { stage } => write!(formatter, "native {stage} worker panicked"),
        }
    }
}

impl std::error::Error for NativeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Emit(error) => Some(error),
            Self::Runtime(error) => Some(error),
            Self::Cache(error) => Some(error),
            Self::Toolchain(error) => Some(error),
            Self::Io { source, .. } | Self::Launch { source, .. } => Some(source),
            Self::WorkerPanic { .. } => None,
        }
    }
}

impl From<EmitError> for NativeError {
    fn from(value: EmitError) -> Self {
        Self::Emit(value)
    }
}

impl From<RuntimeSourceError> for NativeError {
    fn from(value: RuntimeSourceError) -> Self {
        Self::Runtime(value)
    }
}

impl From<CacheError> for NativeError {
    fn from(value: CacheError) -> Self {
        Self::Cache(value)
    }
}

impl From<ToolchainError> for NativeError {
    fn from(value: ToolchainError) -> Self {
        Self::Toolchain(value)
    }
}

/// A configured cache and compiler driver reusable across daemon requests.
#[derive(Debug, Clone)]
pub struct NativeBuilder {
    cache: ArtifactCache,
    toolchain: Toolchain,
}

impl NativeBuilder {
    pub fn discover(cache_root: impl Into<PathBuf>) -> Result<Self, NativeError> {
        Ok(Self::new(
            ArtifactCache::new(cache_root),
            Toolchain::discover()?,
        ))
    }

    /// Reuse one compiler probe for the lifetime of a resident process.
    pub fn discover_cached(cache_root: impl Into<PathBuf>) -> Result<Self, NativeError> {
        let toolchain = DISCOVERED_TOOLCHAIN
            .get_or_init(Toolchain::discover)
            .clone()?;
        Ok(Self::new(ArtifactCache::new(cache_root), toolchain))
    }

    pub const fn new(cache: ArtifactCache, toolchain: Toolchain) -> Self {
        Self { cache, toolchain }
    }

    pub fn toolchain(&self) -> &Toolchain {
        &self.toolchain
    }

    pub fn cache(&self) -> &ArtifactCache {
        &self.cache
    }

    /// Build one bytecode module and its separate host runtime into an executable.
    pub fn build(
        &self,
        program: &Program,
        options: &NativeBuildOptions,
    ) -> Result<NativeBuildReport, NativeError> {
        let total_started = Instant::now();
        let emit_started = Instant::now();
        let module = emit_c(program)?;
        let launcher = launcher_source(&module.symbol)?;
        let emit_elapsed = emit_started.elapsed();
        let runtime = runtime_source();

        let workspace = WorkDirectory::create()?;
        let paths = BuildPaths::new(workspace.path());
        let identity = self.toolchain.identity().clone();
        let profile = options.profile;
        let module_abi = module_abi_identity();
        let runtime_abi = runtime_abi_identity();
        let target = target_identity(&identity);
        let compiler = compiler_identity(&identity);

        let module_source_key = key(
            ArtifactKind::GeneratedC,
            format!("kiln-c-abi-{C_ABI_VERSION}"),
            Backend::C,
            &module_abi,
            &target,
            CacheProfile::Debug,
            Digest::hash(b"deterministic-c11"),
            [Digest::hash(module.source.as_bytes())],
        );
        let (module_source, module_source_digest, generated_c_hit) =
            self.cached_payload(&module_source_key, module.source.as_bytes())?;

        let runtime_source_key = key(
            ArtifactKind::GeneratedC,
            format!("kiln-runtime-{NATIVE_RUNTIME_VERSION}"),
            Backend::Custom("native-runtime-source".to_owned()),
            &runtime_abi,
            &target,
            CacheProfile::Debug,
            Digest::hash(b"portable-runtime-c11"),
            [Digest::hash(runtime.as_bytes())],
        );
        let (runtime_bytes, runtime_source_digest, _) =
            self.cached_payload(&runtime_source_key, runtime.as_bytes())?;

        let launcher_source_key = key(
            ArtifactKind::GeneratedC,
            format!("kiln-launcher-{NATIVE_RUNTIME_VERSION}"),
            Backend::Custom("native-launcher-source".to_owned()),
            &runtime_abi,
            &target,
            CacheProfile::Debug,
            Digest::hash(b"portable-launcher-c11"),
            [Digest::hash(launcher.as_bytes())],
        );
        let (launcher_bytes, launcher_source_digest, _) =
            self.cached_payload(&launcher_source_key, launcher.as_bytes())?;

        let object_options =
            Digest::hash(format!("c11\0{}\0{}", identity.kind, profile.as_str()).as_bytes());
        let application_key = key(
            ArtifactKind::NativeObject,
            &compiler,
            Backend::C,
            &module_abi,
            &target,
            profile.cache(),
            object_options,
            [module_source_digest],
        );
        let runtime_key = key(
            ArtifactKind::RuntimeLibrary,
            &compiler,
            Backend::Custom("native-runtime".to_owned()),
            &runtime_abi,
            &target,
            profile.cache(),
            object_options,
            [runtime_source_digest],
        );
        let launcher_key = key(
            ArtifactKind::NativeObject,
            &compiler,
            Backend::Custom("native-launcher".to_owned()),
            &runtime_abi,
            &target,
            profile.cache(),
            object_options,
            [launcher_source_digest],
        );

        let compile_options = CompileOptions {
            profile: profile.toolchain(),
            ..CompileOptions::default()
        };
        let compile_started = Instant::now();
        let (application_result, runtime_result, launcher_result) = std::thread::scope(|scope| {
            let application = scope.spawn(|| {
                self.object(
                    &application_key,
                    &module_source,
                    &paths.module_c,
                    &paths.module_o,
                    &compile_options,
                )
            });
            let runtime = scope.spawn(|| {
                self.object(
                    &runtime_key,
                    &runtime_bytes,
                    &paths.runtime_c,
                    &paths.runtime_o,
                    &compile_options,
                )
            });
            let launcher = scope.spawn(|| {
                self.object(
                    &launcher_key,
                    &launcher_bytes,
                    &paths.launcher_c,
                    &paths.launcher_o,
                    &compile_options,
                )
            });
            (
                join_worker(application.join(), "application compile"),
                join_worker(runtime.join(), "runtime compile"),
                join_worker(launcher.join(), "launcher compile"),
            )
        });
        let (application, application_digest, application_hit, _application_elapsed) =
            application_result??;
        let (runtime_object, runtime_digest, runtime_hit, _runtime_elapsed) = runtime_result??;
        let (launcher_object, launcher_digest, launcher_hit, _launcher_elapsed) = launcher_result??;
        let compile_elapsed = if application_hit && runtime_hit && launcher_hit {
            Duration::ZERO
        } else {
            compile_started.elapsed()
        };

        let link_options_digest =
            Digest::hash(format!("{}\0system-math", profile.as_str()).as_bytes());
        let executable_key = key(
            ArtifactKind::LinkedArtifact,
            &compiler,
            Backend::C,
            &runtime_abi,
            &target,
            profile.cache(),
            link_options_digest,
            [application_digest, runtime_digest, launcher_digest],
        );
        let (executable, executable_digest, executable_hit, link_elapsed) =
            match self.cache.lookup(&executable_key)? {
                Lookup::Hit(artifact) => (artifact.bytes, artifact.digest, true, Duration::ZERO),
                Lookup::Miss => {
                    materialize(&paths.module_o, &application, false)?;
                    materialize(&paths.runtime_o, &runtime_object, false)?;
                    materialize(&paths.launcher_o, &launcher_object, false)?;
                    let mut link_options = LinkOptions {
                        profile: profile.toolchain(),
                        ..LinkOptions::default()
                    };
                    if cfg!(not(target_os = "windows")) {
                        link_options.libraries.push("m".to_owned());
                    }
                    let report = self.toolchain.link_executable(
                        &[
                            paths.module_o.clone(),
                            paths.runtime_o.clone(),
                            paths.launcher_o.clone(),
                        ],
                        &paths.executable,
                        &link_options,
                    )?;
                    let bytes = read(&paths.executable, "read linked executable")?;
                    let digest = Digest::hash(&bytes);
                    self.cache.put(&executable_key, &bytes)?;
                    (bytes, digest, false, report.elapsed)
                }
            };
        debug_assert_eq!(executable_digest, Digest::hash(&executable));
        materialize(&options.output, &executable, true)?;

        Ok(NativeBuildReport {
            output: options.output.clone(),
            module_symbol: module.symbol,
            tool: identity,
            cache_hits: CacheHits {
                generated_c: generated_c_hit,
                application_object: application_hit,
                runtime_object: runtime_hit,
                launcher_object: launcher_hit,
                executable: executable_hit,
            },
            sizes: ArtifactSizes {
                generated_c: module_source.len() as u64,
                application_object: application.len() as u64,
                runtime_object: runtime_object.len() as u64,
                launcher_object: launcher_object.len() as u64,
                executable: executable.len() as u64,
            },
            timings: BuildTimings {
                emit: emit_elapsed,
                compile: compile_elapsed,
                link: link_elapsed,
                total: total_started.elapsed(),
            },
        })
    }

    /// Build and execute one native module with captured output.
    pub fn run(
        &self,
        program: &Program,
        options: &NativeBuildOptions,
    ) -> Result<NativeRunReport, NativeError> {
        // A public output path is stable and therefore shareable between requests. Launching it
        // directly would let another concurrent build replace the file between build and exec.
        // Build and launch a process-owned copy, then publish the same bytes afterward.
        let workspace = WorkDirectory::create()?;
        let private_output = workspace.path().join(if cfg!(windows) {
            "run-program.exe"
        } else {
            "run-program"
        });
        let mut private_options = options.clone();
        private_options.output = private_output.clone();
        let mut build = self.build(program, &private_options)?;
        let output =
            Command::new(&private_output)
                .output()
                .map_err(|source| NativeError::Launch {
                    executable: private_output.clone(),
                    source,
                })?;
        let executable = read(&private_output, "read private native executable")?;
        materialize(&options.output, &executable, true)?;
        build.output = options.output.clone();
        Ok(NativeRunReport {
            build,
            exit_code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn cached_payload(
        &self,
        key: &ArtifactKey,
        expected: &[u8],
    ) -> Result<(Vec<u8>, Digest, bool), NativeError> {
        match self.cache.lookup(key)? {
            Lookup::Hit(artifact) => Ok((artifact.bytes, artifact.digest, true)),
            Lookup::Miss => {
                let stored = self.cache.put(key, expected)?;
                Ok((expected.to_vec(), stored.digest, false))
            }
        }
    }

    fn object(
        &self,
        key: &ArtifactKey,
        source: &[u8],
        source_path: &Path,
        object_path: &Path,
        options: &CompileOptions,
    ) -> Result<(Vec<u8>, Digest, bool, Duration), NativeError> {
        match self.cache.lookup(key)? {
            Lookup::Hit(artifact) => Ok((artifact.bytes, artifact.digest, true, Duration::ZERO)),
            Lookup::Miss => {
                materialize(source_path, source, false)?;
                let report = self
                    .toolchain
                    .compile_c(source_path, object_path, options)?;
                let bytes = read(object_path, "read native object")?;
                let stored = self.cache.put(key, &bytes)?;
                Ok((bytes, stored.digest, false, report.elapsed))
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn key(
    kind: ArtifactKind,
    compiler: impl Into<String>,
    backend: Backend,
    abi: &str,
    target: &str,
    profile: CacheProfile,
    options: Digest,
    inputs: impl IntoIterator<Item = Digest>,
) -> ArtifactKey {
    ArtifactKey::new(
        kind, compiler, backend, abi, target, "generic", profile, "none", options, inputs,
    )
}

fn module_abi_identity() -> String {
    format!("host-{HOST_ABI_VERSION};c-{C_ABI_VERSION}")
}

fn runtime_abi_identity() -> String {
    format!("{};runtime-{NATIVE_RUNTIME_VERSION}", module_abi_identity())
}

fn target_identity(identity: &ToolIdentity) -> String {
    let sdk_root = std::env::var("SDKROOT").unwrap_or_default();
    let deployment = std::env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_default();
    format!(
        "compiler-target={};host={}-{};sdk={sdk_root};deployment={deployment}",
        identity.target,
        std::env::consts::ARCH,
        std::env::consts::OS,
    )
}

fn compiler_identity(identity: &ToolIdentity) -> String {
    identity.stable_key()
}

fn join_worker<T>(result: std::thread::Result<T>, stage: &'static str) -> Result<T, NativeError> {
    result.map_err(|_| NativeError::WorkerPanic { stage })
}

fn read(path: &Path, operation: &'static str) -> Result<Vec<u8>, NativeError> {
    fs::read(path).map_err(|source| NativeError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })
}

fn materialize(path: &Path, bytes: &[u8], executable: bool) -> Result<(), NativeError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| NativeError::Io {
        operation: "create output directory",
        path: parent.to_path_buf(),
        source,
    })?;
    let temporary = parent.join(format!(
        ".{}.{}-{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("kiln-artifact"),
        std::process::id(),
        MATERIALIZE_COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(|source| NativeError::Io {
                operation: "create temporary artifact",
                path: temporary.clone(),
                source,
            })?;
        file.write_all(bytes).map_err(|source| NativeError::Io {
            operation: "write temporary artifact",
            path: temporary.clone(),
            source,
        })?;
        file.flush().map_err(|source| NativeError::Io {
            operation: "flush temporary artifact",
            path: temporary.clone(),
            source,
        })?;
        if executable {
            set_executable(&temporary)?;
        }
        drop(file);
        install_materialized(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn install_materialized(temporary: &Path, destination: &Path) -> Result<(), NativeError> {
    match fs::rename(temporary, destination) {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(_first_error) if destination.is_file() => {
            // `std::fs::rename` cannot replace a file on Windows. This is an explicitly requested
            // output path, so remove that one exact file and immediately install the completed
            // temporary artifact. Native runs launch their private copy before this publication.
            fs::remove_file(destination).map_err(|source| NativeError::Io {
                operation: "replace existing artifact",
                path: destination.to_path_buf(),
                source,
            })?;
            fs::rename(temporary, destination).map_err(|source| NativeError::Io {
                operation: "install artifact",
                path: destination.to_path_buf(),
                source,
            })
        }
        Err(source) => Err(NativeError::Io {
            operation: "install artifact",
            path: destination.to_path_buf(),
            source,
        }),
    }
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), NativeError> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = fs::metadata(path)
        .map_err(|source| NativeError::Io {
            operation: "read artifact permissions",
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|source| NativeError::Io {
        operation: "make artifact executable",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), NativeError> {
    Ok(())
}

struct WorkDirectory {
    path: PathBuf,
}

impl WorkDirectory {
    fn create() -> Result<Self, NativeError> {
        let counter = WORK_COUNTER.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "kiln-native-{}-{counter}-{timestamp}",
            std::process::id()
        ));
        fs::create_dir(&path).map_err(|source| NativeError::Io {
            operation: "create native work directory",
            path: path.clone(),
            source,
        })?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WorkDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct BuildPaths {
    module_c: PathBuf,
    module_o: PathBuf,
    runtime_c: PathBuf,
    runtime_o: PathBuf,
    launcher_c: PathBuf,
    launcher_o: PathBuf,
    executable: PathBuf,
}

impl BuildPaths {
    fn new(root: &Path) -> Self {
        Self {
            module_c: root.join("module.c"),
            module_o: root.join("module.o"),
            runtime_c: root.join("runtime.c"),
            runtime_o: root.join("runtime.o"),
            launcher_c: root.join("launcher.c"),
            launcher_o: root.join("launcher.o"),
            executable: root.join(if cfg!(windows) {
                "program.exe"
            } else {
                "program"
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_ir::{BYTECODE_VERSION, Constant, HostOp, Instruction};
    use std::sync::{Arc, Barrier};

    #[test]
    fn a_second_build_reuses_every_material_artifact_and_runs() {
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native integration: no C compiler found");
            return;
        };
        let root = std::env::temp_dir().join(format!(
            "kiln-native-test-{}-{}",
            std::process::id(),
            WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let cache = root.join("cache");
        let first_output = root.join("first");
        let second_output = root.join("second");
        let builder = NativeBuilder::new(ArtifactCache::new(cache), toolchain);
        let program = Program {
            version: BYTECODE_VERSION,
            constants: vec![
                Constant::String("native".to_owned()),
                Constant::Number(42.0_f64.to_bits()),
            ],
            code: vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 2,
                },
                Instruction::Return,
            ],
            local_count: 0,
        };

        let first = builder
            .build(
                &program,
                &NativeBuildOptions {
                    output: first_output,
                    profile: NativeProfile::Dev,
                },
            )
            .expect("first native build succeeds");
        assert!(!first.cache_hits.application_object);
        assert!(!first.cache_hits.executable);

        let second = builder
            .run(
                &program,
                &NativeBuildOptions {
                    output: second_output,
                    profile: NativeProfile::Dev,
                },
            )
            .expect("cached native build and run succeeds");
        assert!(second.build.cache_hits.generated_c);
        assert!(second.build.cache_hits.application_object);
        assert!(second.build.cache_hits.runtime_object);
        assert!(second.build.cache_hits.launcher_object);
        assert!(second.build.cache_hits.executable);
        assert_eq!(second.exit_code, Some(0));
        assert_eq!(second.stdout, b"native 42\n");
        assert!(second.stderr.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_runs_with_one_public_output_launch_their_private_program() {
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native concurrency integration: no C compiler found");
            return;
        };
        let root = std::env::temp_dir().join(format!(
            "kiln-native-race-test-{}-{}",
            std::process::id(),
            WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let shared_output = root.join("shared-program");
        let builder = NativeBuilder::new(ArtifactCache::new(root.join("cache")), toolchain);
        let programs = [stdout_program("alpha"), stdout_program("beta")];
        for program in &programs {
            builder
                .build(
                    program,
                    &NativeBuildOptions {
                        output: shared_output.clone(),
                        profile: NativeProfile::Dev,
                    },
                )
                .expect("prime each native executable");
        }

        let barrier = Arc::new(Barrier::new(12));
        let mut threads = Vec::new();
        for index in 0..12 {
            let builder = builder.clone();
            let barrier = Arc::clone(&barrier);
            let program = programs[index % programs.len()].clone();
            let output = shared_output.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                builder
                    .run(
                        &program,
                        &NativeBuildOptions {
                            output,
                            profile: NativeProfile::Dev,
                        },
                    )
                    .expect("concurrent native run succeeds")
                    .stdout
            }));
        }
        for (index, thread) in threads.into_iter().enumerate() {
            let expected: &[u8] = if index % 2 == 0 {
                b"alpha\n"
            } else {
                b"beta\n"
            };
            assert_eq!(thread.join().expect("run thread does not panic"), expected);
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn native_and_interpreter_agree_across_the_scalar_control_flow_slice() {
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native equivalence integration: no C compiler found");
            return;
        };
        let root = std::env::temp_dir().join(format!(
            "kiln-native-equivalence-test-{}-{}",
            std::process::id(),
            WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let builder = NativeBuilder::new(ArtifactCache::new(root.join("cache")), toolchain);

        for (name, program) in scalar_equivalence_programs() {
            let mut host = kiln_runtime::CaptureHost::new();
            kiln_interpreter::execute(&program, &mut host, Default::default())
                .unwrap_or_else(|error| panic!("interpreter fixture {name} failed: {error}"));
            let expected = host.into_stdout().into_bytes();
            let native = builder
                .run(
                    &program,
                    &NativeBuildOptions {
                        output: root.join(name),
                        profile: NativeProfile::Dev,
                    },
                )
                .unwrap_or_else(|error| panic!("native fixture {name} failed: {error}"));
            assert_eq!(native.exit_code, Some(0), "fixture {name}");
            assert_eq!(native.stdout, expected, "fixture {name}");
            assert!(native.stderr.is_empty(), "fixture {name}");
        }
        let _ = fs::remove_dir_all(root);
    }

    fn stdout_program(text: &str) -> Program {
        Program {
            version: BYTECODE_VERSION,
            constants: vec![Constant::String(text.to_owned())],
            code: vec![
                Instruction::Const(0),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Return,
            ],
            local_count: 0,
        }
    }

    fn scalar_equivalence_programs() -> Vec<(&'static str, Program)> {
        vec![
            (
                "loop",
                Program {
                    version: BYTECODE_VERSION,
                    constants: vec![
                        Constant::Number(0.0_f64.to_bits()),
                        Constant::Number(3.0_f64.to_bits()),
                        Constant::Number(1.0_f64.to_bits()),
                        Constant::String("loop".to_owned()),
                    ],
                    code: vec![
                        Instruction::Const(0),
                        Instruction::StoreLocal(0),
                        Instruction::LoadLocal(0),
                        Instruction::Const(1),
                        Instruction::Binary(kiln_ir::BinaryOp::LessThan),
                        Instruction::JumpIfFalse(11),
                        Instruction::LoadLocal(0),
                        Instruction::Const(2),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::StoreLocal(0),
                        Instruction::Jump(2),
                        Instruction::Const(3),
                        Instruction::LoadLocal(0),
                        Instruction::HostCall {
                            op: HostOp::WriteStdout,
                            argc: 2,
                        },
                        Instruction::Return,
                    ],
                    local_count: 1,
                },
            ),
            (
                "concat",
                Program {
                    version: BYTECODE_VERSION,
                    constants: vec![
                        Constant::String("value=".to_owned()),
                        Constant::Number(40.0_f64.to_bits()),
                        Constant::Number(2.0_f64.to_bits()),
                    ],
                    code: vec![
                        Instruction::Const(0),
                        Instruction::Const(1),
                        Instruction::Const(2),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::HostCall {
                            op: HostOp::WriteStdout,
                            argc: 1,
                        },
                        Instruction::Return,
                    ],
                    local_count: 0,
                },
            ),
            (
                "utf16-order",
                Program {
                    version: BYTECODE_VERSION,
                    constants: vec![
                        Constant::String("😀".to_owned()),
                        Constant::String("\u{e000}".to_owned()),
                    ],
                    code: vec![
                        Instruction::Const(0),
                        Instruction::Const(1),
                        Instruction::Binary(kiln_ir::BinaryOp::LessThan),
                        Instruction::HostCall {
                            op: HostOp::WriteStdout,
                            argc: 1,
                        },
                        Instruction::Return,
                    ],
                    local_count: 0,
                },
            ),
            (
                "special-numbers",
                Program {
                    version: BYTECODE_VERSION,
                    constants: vec![
                        Constant::Number(f64::NAN.to_bits()),
                        Constant::Number(f64::INFINITY.to_bits()),
                        Constant::Number((-0.0_f64).to_bits()),
                    ],
                    code: vec![
                        Instruction::Const(0),
                        Instruction::Const(1),
                        Instruction::Const(2),
                        Instruction::HostCall {
                            op: HostOp::WriteStdout,
                            argc: 3,
                        },
                        Instruction::Return,
                    ],
                    local_count: 0,
                },
            ),
        ]
    }
}
