//! Cached orchestration for Kiln's C native backend.
//!
//! The backend, runtime, cache, and platform toolchain deliberately live in separate crates.
//! This crate is the narrow coordinator that gives those pieces an ordered build graph:
//! generated module C, application object, reusable runtime object, launcher object, executable.

mod number_provider;

use std::fmt;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use kiln_artifact_cache::{
    ArtifactCache, ArtifactKey, ArtifactKind, Backend, BuildProfile as CacheProfile, CacheError,
    Digest, Lookup,
};
use kiln_backend_c::{C_ABI_VERSION, EmitError, emit_c, emit_module_with_logical_path};
use kiln_host_abi::HOST_ABI_VERSION;
pub use kiln_host_abi::HostValue;
use kiln_ir::Program;
use kiln_module_graph::{ModuleGraph, ModuleGraphError, ResolvedExport};
use kiln_native_runtime::{
    EMBEDDING_C_ABI_VERSION, EMBEDDING_HOST_ABI_VERSION, NATIVE_RESULT_PATH_ENV,
    NATIVE_RUNTIME_VERSION, RuntimeSourceError, decode_native_result, launcher_source,
    runtime_source,
};
pub use kiln_native_runtime::{embedding_header, embedding_module_header};
use kiln_native_toolchain::{
    BuildProfile as ToolchainProfile, CompileOptions, CompilerKind, LinkOptions, ToolIdentity,
    Toolchain, ToolchainError,
};
use number_provider::{NumberProvider, PROVIDER_ABI_VERSION, PROVIDER_SCHEMA_VERSION};

static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);
static MATERIALIZE_COUNTER: AtomicU64 = AtomicU64::new(0);
static DISCOVERED_TOOLCHAIN: OnceLock<Result<Toolchain, ToolchainError>> = OnceLock::new();
static SHARED_PUBLICATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Inherited variables every supported C driver may consult for C11 search policy.
///
/// `SDKROOT` and `MACOSX_DEPLOYMENT_TARGET` live in [`target_identity`] instead. Ordinary shell
/// variables such as `CFLAGS` and `LDFLAGS` are not included because Kiln invokes the compiler
/// directly rather than through a build system that interprets them.
const C_COMPILER_ENVIRONMENT: [&str; 3] = ["CPATH", "C_INCLUDE_PATH", "LIBRARY_PATH"];
const GCC_COMPILER_ENVIRONMENT: [&str; 2] = ["COMPILER_PATH", "GCC_EXEC_PREFIX"];
const ZIG_COMPILER_ENVIRONMENT: [&str; 1] = ["ZIG_LIB_DIR"];

const _: () = {
    assert!(EMBEDDING_C_ABI_VERSION == C_ABI_VERSION);
    assert!(EMBEDDING_HOST_ABI_VERSION == HOST_ABI_VERSION);
    assert!(PROVIDER_SCHEMA_VERSION == 1);
    assert!(PROVIDER_ABI_VERSION == 1);
};

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
    /// For graph builds, true only when every module's generated C artifact hit.
    pub generated_c: bool,
    /// For graph builds, true only when every module's application object hit.
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

/// Cache facts for one graph module, in deterministic dependency-first graph order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleArtifactInfo {
    /// Project-relative logical path, using `/` separators. `None` is used by the single-module
    /// API, which has no source path in its input contract.
    pub logical_path: Option<String>,
    pub generated_c_cache_hit: bool,
    pub object_cache_hit: bool,
    pub generated_c_bytes: u64,
    pub object_bytes: u64,
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
    /// Per-module cache facts. Graph entries are ordered like `ModuleGraph::module_order`.
    pub module_artifacts: Vec<ModuleArtifactInfo>,
    /// Cache status of the generated runtime and graph-launcher C sources.
    pub runtime_generated_c_cache_hit: bool,
    pub launcher_generated_c_cache_hit: bool,
}

/// Cache facts for an embedded scalar shared library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NativeSharedLibraryCacheHits {
    pub generated_c: bool,
    pub object: bool,
    pub library: bool,
}

/// Byte sizes of an embedded scalar shared library's material products.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NativeSharedLibrarySizes {
    pub generated_c: u64,
    pub object: u64,
    pub library: u64,
}

/// Observable result of one cached embedded scalar shared-library build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSharedLibraryReport {
    pub output: PathBuf,
    /// Canonically named library whose leaf name is the dynamic-loader identity.
    ///
    /// Package this file with the requested output alias and add its parent directory to the
    /// embedding host's dynamic-loader search path.
    pub loader: PathBuf,
    /// Program-specific C header declaring `module_symbol` against the stable embedding ABI.
    pub header: PathBuf,
    pub module_symbol: String,
    pub tool: ToolIdentity,
    pub cache_hits: NativeSharedLibraryCacheHits,
    pub sizes: NativeSharedLibrarySizes,
    pub timings: BuildTimings,
}

/// Captured result of launching a freshly built or cached executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRunReport {
    pub build: NativeBuildReport,
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Exact scalar returned by the module. A failed or signalled process has no result.
    pub result: Option<HostValue>,
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
    Graph(ModuleGraphError),
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
    SymbolCollision {
        symbol: String,
    },
    ArtifactPathCollision {
        first: PathBuf,
        second: PathBuf,
    },
    ImmutableArtifactCollision {
        path: PathBuf,
    },
    NumberProvider {
        message: String,
    },
    RunResult {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for NativeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Emit(error) => write!(formatter, "native emission failed: {error}"),
            Self::Graph(error) => write!(formatter, "native module graph failed: {error}"),
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
            Self::SymbolCollision { symbol } => {
                write!(formatter, "native module symbol collision: {symbol}")
            }
            Self::ArtifactPathCollision { first, second } => write!(
                formatter,
                "native artifacts would overwrite the same path: {} and {}",
                first.display(),
                second.display()
            ),
            Self::ImmutableArtifactCollision { path } => write!(
                formatter,
                "canonical native artifact at {} has different contents",
                path.display()
            ),
            Self::NumberProvider { message } => {
                write!(formatter, "native number provider failed: {message}")
            }
            Self::RunResult { path, message } => write!(
                formatter,
                "native result from {} was invalid: {message}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for NativeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Emit(error) => Some(error),
            Self::Graph(error) => Some(error),
            Self::Runtime(error) => Some(error),
            Self::Cache(error) => Some(error),
            Self::Toolchain(error) => Some(error),
            Self::Io { source, .. } | Self::Launch { source, .. } => Some(source),
            Self::WorkerPanic { .. }
            | Self::SymbolCollision { .. }
            | Self::ArtifactPathCollision { .. }
            | Self::ImmutableArtifactCollision { .. }
            | Self::NumberProvider { .. }
            | Self::RunResult { .. } => None,
        }
    }
}

impl From<EmitError> for NativeError {
    fn from(value: EmitError) -> Self {
        Self::Emit(value)
    }
}

impl From<ModuleGraphError> for NativeError {
    fn from(value: ModuleGraphError) -> Self {
        Self::Graph(value)
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
    number_provider: OnceLock<Result<NumberProvider, String>>,
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
        Self {
            cache,
            toolchain,
            number_provider: OnceLock::new(),
        }
    }

    pub fn toolchain(&self) -> &Toolchain {
        &self.toolchain
    }

    pub fn cache(&self) -> &ArtifactCache {
        &self.cache
    }

    fn number_provider(&self) -> Result<NumberProvider, NativeError> {
        self.number_provider
            .get_or_init(|| {
                number_provider::discover(self.toolchain.identity())
                    .map_err(|error| error.to_string())
            })
            .clone()
            .map_err(|message| NativeError::NumberProvider { message })
    }

    fn cached_number_provider(&self, target: &str) -> Result<(Vec<u8>, Digest, bool), NativeError> {
        let provider = self.number_provider()?;
        let provider_key = key(
            ArtifactKind::RuntimeLibrary,
            &provider.identity,
            Backend::Custom("rust-native-number-provider".to_owned()),
            &native_number_abi_identity(),
            target,
            CacheProfile::Release,
            Digest::hash(
                format!(
                    "schema={PROVIDER_SCHEMA_VERSION};abi={PROVIDER_ABI_VERSION};target={};pic=true;linkage=static",
                    provider.rust_target
                )
                .as_bytes(),
            ),
            [provider.archive_digest],
        );
        self.cached_payload(&provider_key, &provider.archive)
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
        let (number_provider, number_provider_digest, _) = self.cached_number_provider(&target)?;

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
        let (runtime_bytes, runtime_source_digest, runtime_generated_c_cache_hit) =
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
        let (launcher_bytes, launcher_source_digest, launcher_generated_c_cache_hit) =
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
            [
                application_digest,
                runtime_digest,
                launcher_digest,
                number_provider_digest,
            ],
        );
        let (executable, executable_digest, executable_hit, link_elapsed) =
            match self.cache.lookup(&executable_key)? {
                Lookup::Hit(artifact) => (artifact.bytes, artifact.digest, true, Duration::ZERO),
                Lookup::Miss => {
                    materialize(&paths.module_o, &application, false)?;
                    materialize(&paths.runtime_o, &runtime_object, false)?;
                    materialize(&paths.launcher_o, &launcher_object, false)?;
                    materialize(&paths.number_provider_a, &number_provider, false)?;
                    let mut link_options = LinkOptions {
                        profile: profile.toolchain(),
                        ..LinkOptions::default()
                    };
                    if cfg!(not(target_os = "windows")) {
                        link_options.libraries.push("m".to_owned());
                    }
                    link_options
                        .runtime_libraries
                        .push(paths.number_provider_a.clone());
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
            module_artifacts: vec![ModuleArtifactInfo {
                logical_path: None,
                generated_c_cache_hit: generated_c_hit,
                object_cache_hit: application_hit,
                generated_c_bytes: module_source.len() as u64,
                object_bytes: application.len() as u64,
            }],
            runtime_generated_c_cache_hit,
            launcher_generated_c_cache_hit,
        })
    }

    /// Build one scalar bytecode module as a shared library for an embedding host.
    ///
    /// The resulting library exports the module's generated entry point and expects the caller to
    /// provide the stable `kiln_host` callback ABI. It deliberately does not link the CLI runtime
    /// or launcher, so host policy remains in the embedding process.
    pub fn build_shared_library(
        &self,
        program: &Program,
        options: &NativeBuildOptions,
    ) -> Result<NativeSharedLibraryReport, NativeError> {
        let total_started = Instant::now();
        let emit_started = Instant::now();
        let module = emit_c(program)?;
        let header = shared_library_header_path(&options.output);
        if options
            .output
            .extension()
            .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("h"))
        {
            return Err(NativeError::ArtifactPathCollision {
                first: options.output.clone(),
                second: header,
            });
        }
        let header_source = embedding_module_header(&module.symbol)?;
        let emit_elapsed = emit_started.elapsed();

        let workspace = WorkDirectory::create()?;
        let paths = SharedLibraryPaths::new(workspace.path());
        let identity = self.toolchain.identity().clone();
        let profile = options.profile;
        let module_abi = module_abi_identity();
        let embedding_abi = runtime_abi_identity();
        let target = target_identity(&identity);
        let compiler = compiler_identity(&identity);
        let (number_provider, number_provider_digest, _) = self.cached_number_provider(&target)?;
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
        let object_options = Digest::hash(
            format!(
                "shared-library\0pic\0c11\0{}\0{}",
                identity.kind,
                profile.as_str()
            )
            .as_bytes(),
        );
        let object_key = key(
            ArtifactKind::NativeObject,
            &compiler,
            Backend::Custom("native-shared-library".to_owned()),
            &embedding_abi,
            &target,
            profile.cache(),
            object_options,
            [module_source_digest],
        );
        let compile_options = CompileOptions {
            profile: profile.toolchain(),
            position_independent: true,
            ..CompileOptions::default()
        };
        let compile_started = Instant::now();
        let (object, object_digest, object_hit, object_elapsed) = self.object(
            &object_key,
            &module_source,
            &paths.module_c,
            &paths.module_o,
            &compile_options,
        )?;
        let compile_elapsed = if object_hit {
            Duration::ZERO
        } else {
            object_elapsed.max(compile_started.elapsed())
        };

        let link_inputs = [object_digest, number_provider_digest];
        let loader = shared_library_loader_path(
            &options.output,
            shared_library_content_identity(
                &compiler,
                Backend::Custom("native-shared-library-loader".to_owned()),
                &embedding_abi,
                &target,
                profile,
                &module.symbol,
                link_inputs,
            ),
        )?;
        let link_options_digest = Digest::hash(
            format!(
                "shared-library\0{}\0system-math\0{}\0{}\0{}",
                profile.as_str(),
                shared_library_name(),
                shared_library_link_identity(&loader),
                shared_library_link_policy(),
            )
            .as_bytes(),
        );
        let library_key = key(
            ArtifactKind::LinkedArtifact,
            &compiler,
            Backend::Custom("native-shared-library".to_owned()),
            &embedding_abi,
            &target,
            profile.cache(),
            link_options_digest,
            link_inputs,
        );
        let (library, library_hit, link_elapsed) = match self.cache.lookup(&library_key)? {
            Lookup::Hit(artifact) => (artifact.bytes, true, Duration::ZERO),
            Lookup::Miss => {
                materialize(&paths.module_o, &object, false)?;
                materialize(&paths.number_provider_a, &number_provider, false)?;
                let mut link_options = LinkOptions {
                    profile: profile.toolchain(),
                    ..LinkOptions::default()
                };
                if cfg!(not(target_os = "windows")) {
                    link_options.libraries.push("m".to_owned());
                }
                link_options
                    .runtime_libraries
                    .push(paths.number_provider_a.clone());
                configure_shared_library_link(
                    &mut link_options,
                    &loader,
                    &module.symbol,
                    workspace.path(),
                )?;
                let objects = [paths.module_o.clone()];
                let report =
                    self.toolchain
                        .link_shared_library(&objects, &paths.library, &link_options)?;
                let bytes = read(&paths.library, "read linked shared library")?;
                self.cache.put(&library_key, &bytes)?;
                (bytes, false, report.elapsed)
            }
        };
        publish_shared_artifacts(&options.output, &loader, header_source.as_bytes(), &library)?;

        Ok(NativeSharedLibraryReport {
            output: options.output.clone(),
            loader,
            header,
            module_symbol: module.symbol,
            tool: identity,
            cache_hits: NativeSharedLibraryCacheHits {
                generated_c: generated_c_hit,
                object: object_hit,
                library: library_hit,
            },
            sizes: NativeSharedLibrarySizes {
                generated_c: module_source.len() as u64,
                object: object.len() as u64,
                library: library.len() as u64,
            },
            timings: BuildTimings {
                emit: emit_elapsed,
                compile: compile_elapsed,
                link: link_elapsed,
                total: total_started.elapsed(),
            },
        })
    }

    /// Build an acyclic module graph as one embedding shared library.
    ///
    /// The public entry point is a graph-specific two-argument wrapper using the stable embedding
    /// ABI. Individual generated modules retain their internal six-argument ABI; the wrapper
    /// evaluates them in dependency-first order and wires imports from the provider export arrays.
    /// Only the wrapper is declared in the emitted public header. No CLI runtime or launcher is
    /// linked into the resulting library.
    pub fn build_shared_graph(
        &self,
        graph: &ModuleGraph,
        options: &NativeBuildOptions,
    ) -> Result<NativeSharedLibraryReport, NativeError> {
        let total_started = Instant::now();
        let emit_started = Instant::now();
        let mut modules = Vec::with_capacity(graph.module_count());
        let mut symbols = std::collections::HashSet::with_capacity(graph.module_count());
        for node in graph.modules() {
            let logical_path = logical_module_path(graph.project_root(), &node.path)?;
            let emitted = emit_module_with_logical_path(&node.program, &logical_path)?;
            if !symbols.insert(emitted.symbol.clone()) {
                return Err(NativeError::SymbolCollision {
                    symbol: emitted.symbol,
                });
            }
            modules.push((node, emitted));
        }
        if modules.is_empty() {
            return Err(NativeError::Graph(
                ModuleGraphError::InternalMissingModule {
                    path: graph.entry().to_path_buf(),
                },
            ));
        }
        let topology = graph_shared_topology_digest(graph, &modules);
        let wrapper_symbol = format!("kiln_shared_graph_{}", topology.to_hex());
        let wrapper = graph_shared_wrapper_source(graph, &modules, &wrapper_symbol)?;
        let mut header_source = graph_shared_header(&wrapper_symbol)?;
        let emit_elapsed = emit_started.elapsed();

        let header = shared_library_header_path(&options.output);
        if options
            .output
            .extension()
            .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("h"))
        {
            return Err(NativeError::ArtifactPathCollision {
                first: options.output.clone(),
                second: header,
            });
        }
        // Keep the generated header in one owned buffer so publication remains independent of
        // the temporary work directory.
        header_source.shrink_to_fit();

        let workspace = WorkDirectory::create()?;
        let paths = SharedGraphLibraryPaths::new(workspace.path(), modules.len());
        let identity = self.toolchain.identity().clone();
        let profile = options.profile;
        let module_abi = module_abi_identity();
        let embedding_abi = runtime_abi_identity();
        let target = target_identity(&identity);
        let compiler = compiler_identity(&identity);
        let (number_provider, number_provider_digest, _) = self.cached_number_provider(&target)?;
        let mut module_sources = Vec::with_capacity(modules.len());
        for (_, module) in &modules {
            let source_key = key(
                ArtifactKind::GeneratedC,
                format!("kiln-c-abi-{C_ABI_VERSION}"),
                Backend::C,
                &module_abi,
                &target,
                CacheProfile::Debug,
                // PIC affects the object identity below, not this byte-for-byte reusable C unit.
                Digest::hash(b"deterministic-c11-module"),
                [Digest::hash(module.source.as_bytes())],
            );
            module_sources.push(self.cached_payload(&source_key, module.source.as_bytes())?);
        }
        let wrapper_source_key = key(
            ArtifactKind::GeneratedC,
            format!("kiln-shared-graph-wrapper-{NATIVE_RUNTIME_VERSION}"),
            Backend::Custom("native-shared-graph-wrapper-source".to_owned()),
            &embedding_abi,
            &target,
            CacheProfile::Debug,
            Digest::hash(b"deterministic-c11-shared-graph-wrapper"),
            [Digest::hash(wrapper.as_bytes()), topology],
        );
        let (wrapper_bytes, wrapper_source_digest, wrapper_generated_c_hit) =
            self.cached_payload(&wrapper_source_key, wrapper.as_bytes())?;

        let module_object_options = Digest::hash(
            format!(
                "shared-graph\0pic\0c11\0visibility-hidden\0{}\0{}",
                identity.kind,
                profile.as_str()
            )
            .as_bytes(),
        );
        let wrapper_object_options = Digest::hash(
            format!(
                "shared-graph\0pic\0c11\0{}\0{}",
                identity.kind,
                profile.as_str()
            )
            .as_bytes(),
        );
        // Graph module symbols are an internal link protocol. Different shared libraries can
        // contain the same logical path with different bytecode, so exporting those symbols lets
        // a dynamic loader interpose one library's module into another library's wrapper. Keep
        // the six-argument entries externally linkable inside this DSO but hidden outside it.
        let mut module_compile_options = CompileOptions {
            profile: profile.toolchain(),
            position_independent: true,
            ..CompileOptions::default()
        };
        module_compile_options
            .extra_args
            .push("-fvisibility=hidden".into());
        let wrapper_compile_options = CompileOptions {
            profile: profile.toolchain(),
            position_independent: true,
            ..CompileOptions::default()
        };
        let mut module_keys = Vec::with_capacity(modules.len());
        for (index, _) in modules.iter().enumerate() {
            let (_, source_digest, _) = &module_sources[index];
            module_keys.push(key(
                ArtifactKind::NativeObject,
                &compiler,
                Backend::Custom("native-shared-graph-module".to_owned()),
                &embedding_abi,
                &target,
                profile.cache(),
                module_object_options,
                [*source_digest],
            ));
        }
        let wrapper_object_key = key(
            ArtifactKind::NativeObject,
            &compiler,
            Backend::Custom("native-shared-graph-wrapper".to_owned()),
            &embedding_abi,
            &target,
            profile.cache(),
            wrapper_object_options,
            [wrapper_source_digest, topology],
        );
        let compile_started = Instant::now();
        let (module_objects, wrapper_object) = std::thread::scope(|scope| {
            let worker_count = std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .min(modules.len().max(1));
            let chunk_size = modules.len().div_ceil(worker_count).max(1);
            let index_chunks = (0..modules.len())
                .collect::<Vec<_>>()
                .chunks(chunk_size)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let module_keys_ref = &module_keys;
            let module_sources_ref = &module_sources;
            let module_c_ref = &paths.module_c;
            let module_o_ref = &paths.module_o;
            let wrapper_c_ref = &paths.wrapper_c;
            let wrapper_o_ref = &paths.wrapper_o;
            let module_compile_options_ref = &module_compile_options;
            let wrapper_compile_options_ref = &wrapper_compile_options;
            let wrapper_bytes_ref = &wrapper_bytes;
            let module_handles = index_chunks
                .into_iter()
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk
                            .iter()
                            .map(|&index| {
                                let result = self.object(
                                    &module_keys_ref[index],
                                    &module_sources_ref[index].0,
                                    &module_c_ref[index],
                                    &module_o_ref[index],
                                    module_compile_options_ref,
                                );
                                (
                                    index,
                                    result.map(|(object, digest, hit, _elapsed)| {
                                        (object, digest, hit)
                                    }),
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect::<Vec<_>>();
            let wrapper = scope.spawn(move || {
                self.object(
                    &wrapper_object_key,
                    wrapper_bytes_ref,
                    wrapper_c_ref,
                    wrapper_o_ref,
                    wrapper_compile_options_ref,
                )
            });
            let modules = (|| {
                let mut modules = (0..module_keys_ref.len()).map(|_| None).collect::<Vec<_>>();
                for handle in module_handles {
                    for (index, result) in join_worker(handle.join(), "module compile")? {
                        modules[index] = Some(result?);
                    }
                }
                modules
                    .into_iter()
                    .map(|result| {
                        result.ok_or(NativeError::WorkerPanic {
                            stage: "module compile",
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })();
            let module_objects = modules?;
            let wrapper_object = join_worker(wrapper.join(), "shared graph wrapper compile")??;
            Ok::<_, NativeError>((module_objects, wrapper_object))
        })?;
        let compile_elapsed = if module_objects.iter().all(|(_, _, hit)| *hit) && wrapper_object.2 {
            Duration::ZERO
        } else {
            compile_started.elapsed()
        };

        let mut link_inputs = module_objects
            .iter()
            .map(|(_, digest, _)| *digest)
            .collect::<Vec<_>>();
        link_inputs.push(number_provider_digest);
        link_inputs.push(wrapper_object.1);
        let loader = shared_library_loader_path(
            &options.output,
            shared_library_content_identity(
                &compiler,
                Backend::Custom("native-shared-graph-loader".to_owned()),
                &embedding_abi,
                &target,
                profile,
                &wrapper_symbol,
                link_inputs.iter().copied(),
            ),
        )?;
        let link_options_digest = Digest::hash(
            format!(
                "shared-graph\0{}\0system-math\0{}\0{}\0{}",
                profile.as_str(),
                shared_library_name(),
                shared_library_link_identity(&loader),
                shared_library_link_policy(),
            )
            .as_bytes(),
        );
        let library_key = key(
            ArtifactKind::LinkedArtifact,
            &compiler,
            Backend::Custom("native-shared-graph".to_owned()),
            &embedding_abi,
            &target,
            profile.cache(),
            link_options_digest,
            link_inputs.iter().copied(),
        );
        let (library, library_hit, link_elapsed) = match self.cache.lookup(&library_key)? {
            Lookup::Hit(artifact) => (artifact.bytes, true, Duration::ZERO),
            Lookup::Miss => {
                let mut objects = Vec::with_capacity(modules.len() + 1);
                for (index, (object, _, _)) in module_objects.iter().enumerate() {
                    materialize(&paths.module_o[index], object, false)?;
                    objects.push(paths.module_o[index].clone());
                }
                materialize(&paths.number_provider_a, &number_provider, false)?;
                materialize(&paths.wrapper_o, &wrapper_object.0, false)?;
                objects.push(paths.wrapper_o.clone());
                let mut link_options = LinkOptions {
                    profile: profile.toolchain(),
                    ..LinkOptions::default()
                };
                if cfg!(not(target_os = "windows")) {
                    link_options.libraries.push("m".to_owned());
                }
                link_options
                    .runtime_libraries
                    .push(paths.number_provider_a.clone());
                configure_shared_library_link(
                    &mut link_options,
                    &loader,
                    &wrapper_symbol,
                    workspace.path(),
                )?;
                let report =
                    self.toolchain
                        .link_shared_library(&objects, &paths.library, &link_options)?;
                let bytes = read(&paths.library, "read linked shared library")?;
                self.cache.put(&library_key, &bytes)?;
                (bytes, false, report.elapsed)
            }
        };
        publish_shared_artifacts(&options.output, &loader, header_source.as_bytes(), &library)?;

        Ok(NativeSharedLibraryReport {
            output: options.output.clone(),
            loader,
            header,
            module_symbol: wrapper_symbol,
            tool: identity,
            cache_hits: NativeSharedLibraryCacheHits {
                generated_c: module_sources.iter().all(|(_, _, hit)| *hit)
                    && wrapper_generated_c_hit,
                object: module_objects.iter().all(|(_, _, hit)| *hit) && wrapper_object.2,
                library: library_hit,
            },
            sizes: NativeSharedLibrarySizes {
                generated_c: module_sources
                    .iter()
                    .map(|(bytes, _, _)| bytes.len() as u64)
                    .sum::<u64>()
                    + wrapper_bytes.len() as u64,
                object: module_objects
                    .iter()
                    .map(|(bytes, _, _)| bytes.len() as u64)
                    .sum::<u64>()
                    + wrapper_object.0.len() as u64,
                library: library.len() as u64,
            },
            timings: BuildTimings {
                emit: emit_elapsed,
                compile: compile_elapsed,
                link: link_elapsed,
                total: total_started.elapsed(),
            },
        })
    }

    /// Build every module in an acyclic graph into one executable.
    ///
    /// Modules are emitted and compiled independently in the graph's dependency-first order.
    /// The generated launcher then evaluates each module once, wiring each import to the
    /// provider's ordered export array. Consequently, changing one module can reuse unchanged
    /// sibling C and object artifacts from the same cache.
    pub fn build_graph(
        &self,
        graph: &ModuleGraph,
        options: &NativeBuildOptions,
    ) -> Result<NativeBuildReport, NativeError> {
        let total_started = Instant::now();
        let emit_started = Instant::now();
        let mut modules = Vec::with_capacity(graph.module_count());
        let mut logical_paths = Vec::with_capacity(graph.module_count());
        let mut symbols = std::collections::HashSet::with_capacity(graph.module_count());
        for node in graph.modules() {
            let logical_path = logical_module_path(graph.project_root(), &node.path)?;
            let emitted = emit_module_with_logical_path(&node.program, &logical_path)?;
            if !symbols.insert(emitted.symbol.clone()) {
                return Err(NativeError::SymbolCollision {
                    symbol: emitted.symbol,
                });
            }
            logical_paths.push(logical_path);
            modules.push((node, emitted));
        }
        let launcher = graph_launcher_source(graph, &modules)?;
        let emit_elapsed = emit_started.elapsed();
        let runtime = runtime_source();

        let workspace = WorkDirectory::create()?;
        let paths = GraphBuildPaths::new(workspace.path(), modules.len());
        let identity = self.toolchain.identity().clone();
        let profile = options.profile;
        let module_abi = module_abi_identity();
        let runtime_abi = runtime_abi_identity();
        let target = target_identity(&identity);
        let compiler = compiler_identity(&identity);
        let (number_provider, number_provider_digest, _) = self.cached_number_provider(&target)?;

        let mut module_sources = Vec::with_capacity(modules.len());
        for (_, module) in &modules {
            let source_key = key(
                ArtifactKind::GeneratedC,
                format!("kiln-c-abi-{C_ABI_VERSION}"),
                Backend::C,
                &module_abi,
                &target,
                CacheProfile::Debug,
                Digest::hash(b"deterministic-c11-module"),
                [Digest::hash(module.source.as_bytes())],
            );
            module_sources.push(self.cached_payload(&source_key, module.source.as_bytes())?);
        }
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
        let (runtime_bytes, runtime_source_digest, runtime_generated_c_cache_hit) =
            self.cached_payload(&runtime_source_key, runtime.as_bytes())?;
        let launcher_source_key = key(
            ArtifactKind::GeneratedC,
            format!("kiln-graph-launcher-{NATIVE_RUNTIME_VERSION}"),
            Backend::Custom("native-graph-launcher-source".to_owned()),
            &runtime_abi,
            &target,
            CacheProfile::Debug,
            Digest::hash(b"portable-graph-launcher-c11"),
            [
                Digest::hash(launcher.as_bytes()),
                graph_topology_digest(graph, &modules),
            ],
        );
        let (launcher_bytes, launcher_source_digest, launcher_generated_c_cache_hit) =
            self.cached_payload(&launcher_source_key, launcher.as_bytes())?;

        let object_options =
            Digest::hash(format!("c11\0{}\0{}", identity.kind, profile.as_str()).as_bytes());
        let compile_options = CompileOptions {
            profile: profile.toolchain(),
            ..CompileOptions::default()
        };
        let compile_started = Instant::now();
        let mut module_keys = Vec::with_capacity(modules.len());
        for (index, (_, _module)) in modules.iter().enumerate() {
            let (_, source_digest, _) = &module_sources[index];
            module_keys.push(key(
                ArtifactKind::NativeObject,
                &compiler,
                Backend::C,
                &module_abi,
                &target,
                profile.cache(),
                object_options,
                [*source_digest],
            ));
        }
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
            Backend::Custom("native-graph-launcher".to_owned()),
            &runtime_abi,
            &target,
            profile.cache(),
            object_options,
            [launcher_source_digest],
        );
        let (module_objects, runtime_object, launcher_object) = std::thread::scope(|scope| {
            let worker_count = std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .min(modules.len().max(1));
            let chunk_size = modules.len().div_ceil(worker_count).max(1);
            let index_chunks = (0..modules.len())
                .collect::<Vec<_>>()
                .chunks(chunk_size)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let module_keys_ref = &module_keys;
            let module_sources_ref = &module_sources;
            let module_c_ref = &paths.module_c;
            let module_o_ref = &paths.module_o;
            let compile_options_ref = &compile_options;
            let runtime_c_ref = &paths.runtime_c;
            let runtime_o_ref = &paths.runtime_o;
            let launcher_c_ref = &paths.launcher_c;
            let launcher_o_ref = &paths.launcher_o;
            let module_handles = index_chunks
                .into_iter()
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk
                            .iter()
                            .map(|&index| {
                                let object_key = &module_keys_ref[index];
                                let result = self.object(
                                    object_key,
                                    &module_sources_ref[index].0,
                                    &module_c_ref[index],
                                    &module_o_ref[index],
                                    compile_options_ref,
                                );
                                (
                                    index,
                                    result.map(|(object, digest, hit, _elapsed)| {
                                        (object, digest, hit)
                                    }),
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect::<Vec<_>>();
            let runtime = scope.spawn(move || {
                self.object(
                    &runtime_key,
                    &runtime_bytes,
                    runtime_c_ref,
                    runtime_o_ref,
                    compile_options_ref,
                )
            });
            let launcher = scope.spawn(move || {
                self.object(
                    &launcher_key,
                    &launcher_bytes,
                    launcher_c_ref,
                    launcher_o_ref,
                    compile_options_ref,
                )
            });
            let modules = (|| {
                let mut modules = (0..module_keys_ref.len()).map(|_| None).collect::<Vec<_>>();
                for handle in module_handles {
                    for (index, result) in join_worker(handle.join(), "module compile")? {
                        modules[index] = Some(result?);
                    }
                }
                modules
                    .into_iter()
                    .map(|result| {
                        result.ok_or(NativeError::WorkerPanic {
                            stage: "module compile",
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })();
            let module_objects = modules?;
            let runtime_object = join_worker(runtime.join(), "runtime compile")??;
            let launcher_object = join_worker(launcher.join(), "launcher compile")??;
            Ok::<_, NativeError>((module_objects, runtime_object, launcher_object))
        })?;
        let (runtime_object, runtime_digest, runtime_hit, _) = runtime_object;
        let (launcher_object, launcher_digest, launcher_hit, _) = launcher_object;
        let compile_elapsed =
            if module_objects.iter().all(|(_, _, hit)| *hit) && runtime_hit && launcher_hit {
                Duration::ZERO
            } else {
                compile_started.elapsed()
            };

        let link_options_digest =
            Digest::hash(format!("{}\0system-math", profile.as_str()).as_bytes());
        let mut link_inputs = module_objects
            .iter()
            .map(|(_, digest, _)| *digest)
            .collect::<Vec<_>>();
        link_inputs.push(runtime_digest);
        link_inputs.push(launcher_digest);
        link_inputs.push(number_provider_digest);
        let executable_key = key(
            ArtifactKind::LinkedArtifact,
            &compiler,
            Backend::C,
            &runtime_abi,
            &target,
            profile.cache(),
            link_options_digest,
            link_inputs,
        );
        let (executable, executable_digest, executable_hit, link_elapsed) = match self
            .cache
            .lookup(&executable_key)?
        {
            Lookup::Hit(artifact) => (artifact.bytes, artifact.digest, true, Duration::ZERO),
            Lookup::Miss => {
                let mut objects = Vec::with_capacity(modules.len() + 2);
                for (index, (object, _, _)) in module_objects.iter().enumerate() {
                    materialize(&paths.module_o[index], object, false)?;
                    objects.push(paths.module_o[index].clone());
                }
                materialize(&paths.runtime_o, &runtime_object, false)?;
                materialize(&paths.launcher_o, &launcher_object, false)?;
                materialize(&paths.number_provider_a, &number_provider, false)?;
                objects.push(paths.runtime_o.clone());
                objects.push(paths.launcher_o.clone());
                let mut link_options = LinkOptions {
                    profile: profile.toolchain(),
                    ..LinkOptions::default()
                };
                if cfg!(not(target_os = "windows")) {
                    link_options.libraries.push("m".to_owned());
                }
                link_options
                    .runtime_libraries
                    .push(paths.number_provider_a.clone());
                let report =
                    self.toolchain
                        .link_executable(&objects, &paths.executable, &link_options)?;
                let bytes = read(&paths.executable, "read linked executable")?;
                let digest = Digest::hash(&bytes);
                self.cache.put(&executable_key, &bytes)?;
                (bytes, digest, false, report.elapsed)
            }
        };
        debug_assert_eq!(executable_digest, Digest::hash(&executable));
        materialize(&options.output, &executable, true)?;

        let module_artifacts = (0..modules.len())
            .map(|index| {
                let (source, _, source_hit) = &module_sources[index];
                let (object, _, object_hit) = &module_objects[index];
                ModuleArtifactInfo {
                    logical_path: Some(logical_paths[index].clone()),
                    generated_c_cache_hit: *source_hit,
                    object_cache_hit: *object_hit,
                    generated_c_bytes: source.len() as u64,
                    object_bytes: object.len() as u64,
                }
            })
            .collect();

        let entry_symbol = modules
            .iter()
            .find(|(node, _)| node.path == graph.entry())
            .map(|(_, module)| module.symbol.clone())
            .ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::InternalMissingModule {
                    path: graph.entry().to_path_buf(),
                })
            })?;
        Ok(NativeBuildReport {
            output: options.output.clone(),
            module_symbol: entry_symbol,
            tool: identity,
            cache_hits: CacheHits {
                generated_c: module_sources.iter().all(|(_, _, hit)| *hit),
                application_object: module_objects.iter().all(|(_, _, hit)| *hit),
                runtime_object: runtime_hit,
                launcher_object: launcher_hit,
                executable: executable_hit,
            },
            sizes: ArtifactSizes {
                generated_c: module_sources
                    .iter()
                    .map(|(bytes, _, _)| bytes.len() as u64)
                    .sum(),
                application_object: module_objects
                    .iter()
                    .map(|(bytes, _, _)| bytes.len() as u64)
                    .sum(),
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
            module_artifacts,
            runtime_generated_c_cache_hit,
            launcher_generated_c_cache_hit,
        })
    }

    /// Build and execute a module graph with captured output.
    pub fn run_graph(
        &self,
        graph: &ModuleGraph,
        options: &NativeBuildOptions,
    ) -> Result<NativeRunReport, NativeError> {
        let workspace = WorkDirectory::create()?;
        let private_output = workspace.path().join(if cfg!(windows) {
            "run-graph.exe"
        } else {
            "run-graph"
        });
        let result_path = workspace.path().join("result.bin");
        let mut private_options = options.clone();
        private_options.output = private_output.clone();
        let mut build = self.build_graph(graph, &private_options)?;
        let output = Command::new(&private_output)
            .env(NATIVE_RESULT_PATH_ENV, &result_path)
            .output()
            .map_err(|source| NativeError::Launch {
                executable: private_output.clone(),
                source,
            })?;
        let result = decode_run_result(&result_path, output.status.success())?;
        let executable = read(&private_output, "read private native executable")?;
        materialize(&options.output, &executable, true)?;
        build.output = options.output.clone();
        Ok(NativeRunReport {
            build,
            exit_code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
            result,
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
        let result_path = workspace.path().join("result.bin");
        let mut private_options = options.clone();
        private_options.output = private_output.clone();
        let mut build = self.build(program, &private_options)?;
        let output = Command::new(&private_output)
            .env(NATIVE_RESULT_PATH_ENV, &result_path)
            .output()
            .map_err(|source| NativeError::Launch {
                executable: private_output.clone(),
                source,
            })?;
        let result = decode_run_result(&result_path, output.status.success())?;
        let executable = read(&private_output, "read private native executable")?;
        materialize(&options.output, &executable, true)?;
        build.output = options.output.clone();
        Ok(NativeRunReport {
            build,
            exit_code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
            result,
        })
    }

    fn cached_payload(
        &self,
        key: &ArtifactKey,
        expected: &[u8],
    ) -> Result<(Vec<u8>, Digest, bool), NativeError> {
        cached_payload(&self.cache, key, expected)
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

fn cached_payload(
    cache: &ArtifactCache,
    key: &ArtifactKey,
    expected: &[u8],
) -> Result<(Vec<u8>, Digest, bool), NativeError> {
    match cache.lookup(key)? {
        Lookup::Hit(artifact) if artifact.bytes != expected => Err(CacheError::Corrupt {
            path: artifact.path,
            reason: format!(
                "cached payload digest {} does not match deterministic expected digest {}",
                artifact.digest.to_hex(),
                Digest::hash(expected).to_hex()
            ),
        }
        .into()),
        Lookup::Hit(artifact) => Ok((artifact.bytes, artifact.digest, true)),
        Lookup::Miss => {
            let stored = cache.put(key, expected)?;
            Ok((expected.to_vec(), stored.digest, false))
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

fn native_number_abi_identity() -> String {
    format!("kiln-native-number-provider-{PROVIDER_ABI_VERSION}")
}

fn target_identity(identity: &ToolIdentity) -> String {
    let sdk_root = std::env::var_os("SDKROOT");
    let deployment = std::env::var_os("MACOSX_DEPLOYMENT_TARGET");
    target_identity_from_parts(&identity.target, sdk_root.as_deref(), deployment.as_deref())
}

/// Build an injective cache identity for the compiler target and platform settings.
///
/// Environment values are framed as raw bytes, rather than interpolated into a delimiter-based
/// string, so values containing separators (or invalid UTF-8 on Unix) cannot alias one another.
/// The presence marker also keeps an unset variable distinct from a variable set to the empty
/// string. The version prefix intentionally invalidates identities generated by the old format.
fn target_identity_from_parts(
    compiler_target: &str,
    sdk_root: Option<&std::ffi::OsStr>,
    deployment: Option<&std::ffi::OsStr>,
) -> String {
    fn frame(output: &mut String, bytes: &[u8]) {
        write!(output, "{}:", bytes.len()).expect("writing target identity to String");
        for byte in bytes {
            write!(output, "{byte:02x}").expect("writing target identity to String");
        }
    }

    fn field(output: &mut String, name: &str, value: Option<&[u8]>) {
        frame(output, name.as_bytes());
        output.push('=');
        match value {
            Some(value) => {
                output.push('1');
                frame(output, value);
            }
            None => output.push('0'),
        }
        output.push(';');
    }

    let mut identity = String::from("kiln-native-target-v2;");
    field(
        &mut identity,
        "compiler-target",
        Some(compiler_target.as_bytes()),
    );
    field(
        &mut identity,
        "host-arch",
        Some(std::env::consts::ARCH.as_bytes()),
    );
    field(
        &mut identity,
        "host-os",
        Some(std::env::consts::OS.as_bytes()),
    );
    field(
        &mut identity,
        "sdk-root",
        sdk_root.map(std::ffi::OsStr::as_encoded_bytes),
    );
    field(
        &mut identity,
        "deployment-target",
        deployment.map(std::ffi::OsStr::as_encoded_bytes),
    );
    identity
}

fn compiler_identity(identity: &ToolIdentity) -> String {
    compiler_identity_from_parts(
        &identity.stable_key(),
        compiler_environment_digest(identity.kind),
    )
}

fn compiler_identity_from_parts(tool: &str, environment: Digest) -> String {
    format!(
        "{}:{tool}|compiler-environment-sha256:{}",
        tool.len(),
        environment.to_hex()
    )
}

fn compiler_environment_digest(kind: CompilerKind) -> Digest {
    compiler_environment_digest_with(kind, |name| std::env::var_os(name))
}

fn compiler_environment_digest_with(
    kind: CompilerKind,
    mut value: impl FnMut(&str) -> Option<std::ffi::OsString>,
) -> Digest {
    fn frame(output: &mut Vec<u8>, bytes: &[u8]) {
        output.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        output.extend_from_slice(bytes);
    }

    let mut encoded = Vec::new();
    encoded.extend_from_slice(b"kiln-native-compiler-environment-v1\0");
    let driver_environment: &[&str] = match kind {
        CompilerKind::Zig => &ZIG_COMPILER_ENVIRONMENT,
        CompilerKind::Gcc | CompilerKind::Cc => &GCC_COMPILER_ENVIRONMENT,
        CompilerKind::Clang => &[],
    };
    for name in C_COMPILER_ENVIRONMENT
        .into_iter()
        .chain(driver_environment.iter().copied())
    {
        frame(&mut encoded, name.as_bytes());
        match value(name) {
            Some(value) => {
                encoded.push(1);
                frame(&mut encoded, value.as_os_str().as_encoded_bytes());
            }
            None => encoded.push(0),
        }
    }
    Digest::hash(&encoded)
}

fn logical_module_path(root: &Path, path: &Path) -> Result<String, NativeError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        NativeError::Emit(EmitError::InvalidProgram {
            message: format!(
                "module path {} is outside project root {}",
                path.display(),
                root.display()
            ),
        })
    })?;
    let mut logical = String::new();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(NativeError::Emit(EmitError::InvalidProgram {
                message: format!(
                    "module path {} is not a normalized project-relative path",
                    path.display()
                ),
            }));
        };
        let component = component.to_str().ok_or_else(|| {
            NativeError::Emit(EmitError::InvalidProgram {
                message: format!("module path {} is not valid UTF-8", path.display()),
            })
        })?;
        if component.is_empty() || component.contains(['\\', '\0']) {
            return Err(NativeError::Emit(EmitError::InvalidProgram {
                message: format!(
                    "module path {} is not a normalized slash-separated path",
                    path.display()
                ),
            }));
        }
        if !logical.is_empty() {
            logical.push('/');
        }
        logical.push_str(component);
    }
    if logical.is_empty() {
        return Err(NativeError::Emit(EmitError::InvalidProgram {
            message: format!(
                "module path {} has no logical relative path",
                path.display()
            ),
        }));
    }
    Ok(logical)
}

fn graph_topology_digest(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
) -> Digest {
    let mut bytes = Vec::new();
    let mut indices = std::collections::HashMap::with_capacity(modules.len());
    for (index, (node, _)) in modules.iter().enumerate() {
        indices.insert(node.path.clone(), index);
    }
    let entry_index = modules
        .iter()
        .position(|(node, _)| node.path == graph.entry())
        .unwrap_or(usize::MAX);
    bytes.extend_from_slice(entry_index.to_string().as_bytes());
    bytes.push(0);
    for (node, module) in modules {
        bytes.extend_from_slice(module.symbol.as_bytes());
        bytes.push(0);
        for dependency in &node.dependencies {
            bytes.extend_from_slice(dependency.specifier.as_bytes());
            bytes.push(0);
            let index = indices
                .get(&dependency.target)
                .copied()
                .unwrap_or(usize::MAX);
            bytes.extend_from_slice(index.to_string().as_bytes());
            bytes.push(0);
        }
        for import in &node.program.imports {
            bytes.extend_from_slice(import.source.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(import.imported.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(import.local.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(import.local_slot.to_string().as_bytes());
            bytes.push(0);
        }
        for export in &node.program.exports {
            bytes.extend_from_slice(export.exported.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(export.local_slot.to_string().as_bytes());
            bytes.push(0);
        }
    }
    Digest::hash(&bytes)
}

/// Identity of the graph wiring used by the shared wrapper. Local bytecode slot assignments are
/// deliberately excluded: changing a module body may renumber those private slots without
/// changing the provider/import topology or the wrapper source.
fn graph_shared_topology_digest(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
) -> Digest {
    let mut bytes = Vec::new();
    let mut indices = std::collections::HashMap::with_capacity(modules.len());
    for (index, (node, _)) in modules.iter().enumerate() {
        indices.insert(node.path.clone(), index);
    }
    let entry_index = modules
        .iter()
        .position(|(node, _)| node.path == graph.entry())
        .unwrap_or(usize::MAX);
    bytes.extend_from_slice(entry_index.to_string().as_bytes());
    bytes.push(0);
    for (node, module) in modules {
        bytes.extend_from_slice(module.symbol.as_bytes());
        bytes.push(0);
        for dependency in &node.dependencies {
            bytes.extend_from_slice(dependency.specifier.as_bytes());
            bytes.push(0);
            let index = indices
                .get(&dependency.target)
                .copied()
                .unwrap_or(usize::MAX);
            bytes.extend_from_slice(index.to_string().as_bytes());
            bytes.push(0);
        }
        for import in &node.program.imports {
            bytes.extend_from_slice(import.source.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(import.imported.as_bytes());
            bytes.push(0);
        }
        for export in &node.program.exports {
            bytes.extend_from_slice(export.exported.as_bytes());
            bytes.push(0);
        }
    }
    Digest::hash(&bytes)
}

/// Resolve one module import to the concrete export-array slot that owns its binding.
///
/// Raw provider export tables are insufficient here: `export *` deliberately has only one
/// synthetic entry, while the graph linker resolves each forwarded name to its canonical origin.
/// The synthetic namespace import used solely to carry a star table is skipped; user-observable
/// namespace values need an object-capable module ABI and remain explicitly unsupported natively.
fn graph_import_slot(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    indices: &std::collections::HashMap<PathBuf, usize>,
    node: &kiln_module_graph::ModuleNode,
    import: &kiln_ir::ImportBinding,
) -> Result<Option<(usize, usize)>, NativeError> {
    let dependency = node
        .dependencies
        .iter()
        .find(|dependency| dependency.specifier == import.source)
        .ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::UnresolvedImport {
                importer: node.path.clone(),
                source: import.source.clone(),
            })
        })?;
    if import.imported == kiln_ir::NAMESPACE_IMPORT {
        let internal_star_table = node.program.exports.iter().any(|export| {
            export.local_slot == import.local_slot
                && export.exported.starts_with(kiln_ir::STAR_REEXPORT_PREFIX)
        });
        if internal_star_table {
            return Ok(None);
        }
        return Err(NativeError::Emit(EmitError::Unsupported {
            feature: "namespace imports in native module graphs".to_owned(),
        }));
    }

    let (origin, local_slot) = match graph.resolved_export(&dependency.target, &import.imported) {
        Some(ResolvedExport::Local { module, local_slot }) => (module, *local_slot),
        Some(ResolvedExport::Namespace { .. }) => {
            return Err(NativeError::Emit(EmitError::Unsupported {
                feature: "namespace re-exports in native module graphs".to_owned(),
            }));
        }
        Some(ResolvedExport::Ambiguous) => {
            return Err(NativeError::Graph(ModuleGraphError::AmbiguousExport {
                importer: node.path.clone(),
                provider: dependency.target.clone(),
                name: import.imported.clone(),
            }));
        }
        None => {
            return Err(NativeError::Graph(ModuleGraphError::MissingExport {
                importer: node.path.clone(),
                provider: dependency.target.clone(),
                name: import.imported.clone(),
            }));
        }
    };
    let provider_index = *indices.get(origin).ok_or_else(|| {
        NativeError::Graph(ModuleGraphError::InternalMissingModule {
            path: origin.clone(),
        })
    })?;
    let export_index = modules[provider_index]
        .0
        .program
        .exports
        .iter()
        .position(|export| {
            export.local_slot == local_slot
                && !export.exported.starts_with(kiln_ir::STAR_REEXPORT_PREFIX)
        })
        .ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::MissingExport {
                importer: node.path.clone(),
                provider: origin.clone(),
                name: import.imported.clone(),
            })
        })?;
    Ok(Some((provider_index, export_index)))
}

fn graph_launcher_source(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
) -> Result<String, NativeError> {
    let mut indices = std::collections::HashMap::with_capacity(modules.len());
    for (index, (node, _)) in modules.iter().enumerate() {
        indices.insert(node.path.clone(), index);
    }
    let mut output = String::with_capacity(2048 + modules.len() * 512);
    output.push_str(
        r#"/* Kiln native graph launcher. */
#include <locale.h>
#include <stddef.h>
#include <stdint.h>

enum kiln_value_tag {
    KILN_VALUE_UNDEFINED = 0,
    KILN_VALUE_NULL = 1,
    KILN_VALUE_BOOLEAN = 2,
    KILN_VALUE_NUMBER = 3,
    KILN_VALUE_STRING = 4
};

typedef struct kiln_value {
    uint32_t tag;
    uint32_t reserved;
    uint64_t bits;
    const char *ptr;
    size_t len;
} kiln_value;

typedef int (*kiln_host_call_fn)(void *context, uint16_t operation, uint16_t argc,
                                 const kiln_value *arguments, kiln_value *result);
typedef struct kiln_host {
    kiln_host_call_fn call;
    void *context;
} kiln_host;

extern int kiln_cli_host_call(void *context, uint16_t operation, uint16_t argc,
                              const kiln_value *arguments, kiln_value *result);
extern int kiln_cli_write_result(kiln_value value);
extern void kiln_cli_report_failure(int status, kiln_value value);
"#,
    );
    for (_, module) in modules {
        writeln!(
            output,
            "extern int {}(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count);",
            module.symbol
        )
        .unwrap();
    }
    for (index, (node, _)) in modules.iter().enumerate() {
        if !node.program.exports.is_empty() {
            writeln!(
                output,
                "static kiln_value kiln_exports_{index}[{}];",
                node.program.exports.len()
            )
            .unwrap();
        }
        if !node.program.imports.is_empty() {
            writeln!(
                output,
                "static kiln_value kiln_imports_{index}[{}];",
                node.program.imports.len()
            )
            .unwrap();
        }
    }
    output.push_str(
        r#"
int main(void) {
    kiln_host host;
    kiln_value result;
    int status;
"#,
    );
    output.push_str(
        r#"    if (setlocale(LC_NUMERIC, "C") == NULL) {
        return 1;
    }
    host.call = kiln_cli_host_call;
    host.context = NULL;
"#,
    );
    for (index, (node, module)) in modules.iter().enumerate() {
        for (import_index, import) in node.program.imports.iter().enumerate() {
            let Some((provider_index, export_index)) =
                graph_import_slot(graph, modules, &indices, node, import)?
            else {
                continue;
            };
            writeln!(
                output,
                "    kiln_imports_{index}[{import_index}] = kiln_exports_{provider_index}[{export_index}];"
            )
            .unwrap();
        }
        let imports = if node.program.imports.is_empty() {
            "NULL".to_owned()
        } else {
            format!("kiln_imports_{index}")
        };
        let exports = if node.program.exports.is_empty() {
            "NULL".to_owned()
        } else {
            format!("kiln_exports_{index}")
        };
        writeln!(
            output,
            "    status = {}(&host, &result, {}, (size_t){}, {}, (size_t){});",
            module.symbol,
            imports,
            node.program.imports.len(),
            exports,
            node.program.exports.len()
        )
        .unwrap();
        // A unit that stops says why. An uncaught throw arrives as text in `result`, because a
        // reference cannot cross the boundary and printing it is the only thing left to do with
        // it -- exiting 1 in silence is the failure this corpus exists to make impossible.
        output.push_str(
            "    if (status != 0) { kiln_cli_report_failure(status, result); return 1; }\n",
        );
        if node.path == graph.entry() {
            output.push_str("    if (kiln_cli_write_result(result) != 0) return 1;\n");
        }
    }
    output.push_str("    return 0;\n}\n");
    // Keep this check in the launcher generation path so an unexpectedly empty graph fails as a
    // structured graph error instead of producing a linker error with little context.
    if modules.is_empty() {
        return Err(NativeError::Graph(
            ModuleGraphError::InternalMissingModule {
                path: graph.entry().to_path_buf(),
            },
        ));
    }
    Ok(output)
}

fn graph_shared_header(wrapper_symbol: &str) -> Result<String, NativeError> {
    // The graph wrapper has the same public two-argument signature as a scalar embedded module,
    // so reuse the runtime's canonical symbol validation and include-guarded header generator.
    embedding_module_header(wrapper_symbol).map_err(NativeError::from)
}

fn graph_shared_wrapper_source(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    wrapper_symbol: &str,
) -> Result<String, NativeError> {
    let mut indices = std::collections::HashMap::with_capacity(modules.len());
    for (index, (node, _)) in modules.iter().enumerate() {
        indices.insert(node.path.clone(), index);
    }
    let entry_index = modules
        .iter()
        .position(|(node, _)| node.path == graph.entry())
        .ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::InternalMissingModule {
                path: graph.entry().to_path_buf(),
            })
        })?;
    let mut output = String::with_capacity(embedding_header().len() + modules.len() * 768);
    output.push_str("/* Kiln native shared graph wrapper. */\n");
    output.push_str(
        "#include <stddef.h>\n#include <stdint.h>\n#include <stdlib.h>\n#include <string.h>\n\n",
    );
    output.push_str(embedding_header());
    output.push('\n');
    for (_, module) in modules {
        writeln!(
            output,
            "extern int {}(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count);",
            module.symbol
        )
        .unwrap();
    }
    output.push_str(
        r#"
typedef struct kiln_proxy_arena {
    void **allocations;
    size_t count;
    size_t capacity;
} kiln_proxy_arena;

typedef struct kiln_proxy_host {
    kiln_host *upstream;
    kiln_proxy_arena *arena;
} kiln_proxy_host;

static void kiln_proxy_arena_clear(kiln_proxy_arena *arena) {
    size_t index;
    if (arena == NULL) return;
    for (index = 0; index < arena->count; ++index) free(arena->allocations[index]);
    free(arena->allocations);
    arena->allocations = NULL;
    arena->count = 0;
    arena->capacity = 0;
}

static int kiln_proxy_arena_track(kiln_proxy_arena *arena, void *allocation) {
    size_t capacity;
    void **grown;
    if (arena == NULL || allocation == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (arena->count == arena->capacity) {
        capacity = arena->capacity == 0 ? 8 : arena->capacity;
        if (capacity > (size_t)-1 / 2) return KILN_C99_HOST_ERROR;
        capacity *= 2;
        if (capacity > (size_t)-1 / sizeof(*grown)) return KILN_C99_HOST_ERROR;
        grown = (void **)realloc(arena->allocations, capacity * sizeof(*grown));
        if (grown == NULL) return KILN_C99_HOST_ERROR;
        arena->allocations = grown;
        arena->capacity = capacity;
    }
    arena->allocations[arena->count++] = allocation;
    return KILN_C99_OK;
}

static int kiln_proxy_call(void *context, uint16_t operation, uint16_t argc,
                           const kiln_value *arguments, kiln_value *result) {
    kiln_proxy_host *proxy = (kiln_proxy_host *)context;
    kiln_host *upstream;
    kiln_proxy_arena *arena;
    char *copy;
    int status;
    if (proxy == NULL || proxy->upstream == NULL || proxy->upstream->call == NULL ||
        result == NULL) return KILN_C99_INVALID_ARGUMENT;
    upstream = proxy->upstream;
    arena = proxy->arena;
    status = upstream->call(upstream->context, operation, argc, arguments, result);
    if (status != KILN_C99_OK) return status;
    if (result->tag != KILN_VALUE_STRING) return KILN_C99_OK;
    if (result->len == (size_t)-1 || (result->len != 0 && result->ptr == NULL))
        return KILN_C99_INVALID_ARGUMENT;
    copy = (char *)malloc(result->len == 0 ? 1 : result->len);
    if (copy == NULL) return KILN_C99_HOST_ERROR;
    if (result->len != 0) memcpy(copy, result->ptr, result->len);
    status = kiln_proxy_arena_track(arena, copy);
    if (status != KILN_C99_OK) {
        free(copy);
        return status;
    }
    result->ptr = copy;
    return KILN_C99_OK;
}
"#,
    );
    writeln!(
        output,
        "\nint {wrapper_symbol}(kiln_host *host, kiln_value *result) {{\n    kiln_proxy_arena arena = {{ NULL, 0, 0 }};\n    kiln_proxy_host proxy = {{ host, &arena }};\n    kiln_host proxy_host = {{ kiln_proxy_call, &proxy }};\n    kiln_value **import_arrays = NULL;\n    kiln_value **export_arrays = NULL;\n    kiln_value module_result;\n    int status = KILN_C99_OK;\n    size_t index;\n    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n    *result = (kiln_value){{ KILN_VALUE_UNDEFINED, 0, 0, NULL, 0 }};\n    if ((size_t){count} > (size_t)-1 / sizeof(*import_arrays) ||\n        (size_t){count} > (size_t)-1 / sizeof(*export_arrays)) {{\n        status = KILN_C99_HOST_ERROR;\n        goto cleanup;\n    }}\n    import_arrays = (kiln_value **)calloc((size_t){count}, sizeof(*import_arrays));\n    export_arrays = (kiln_value **)calloc((size_t){count}, sizeof(*export_arrays));\n    if (import_arrays == NULL || export_arrays == NULL) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}",
        wrapper_symbol = wrapper_symbol,
        count = modules.len(),
    )
    .unwrap();
    for (index, (node, _)) in modules.iter().enumerate() {
        if !node.program.imports.is_empty() {
            writeln!(
                output,
                "    if ((size_t){} > (size_t)-1 / sizeof(**import_arrays)) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}\n    import_arrays[{index}] = (kiln_value *)calloc((size_t){}, sizeof(**import_arrays));\n    if (import_arrays[{index}] == NULL) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}",
                node.program.imports.len(),
                node.program.imports.len()
            )
            .unwrap();
        }
        if !node.program.exports.is_empty() {
            writeln!(
                output,
                "    if ((size_t){} > (size_t)-1 / sizeof(**export_arrays)) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}\n    export_arrays[{index}] = (kiln_value *)malloc((size_t){} * sizeof(**export_arrays));\n    if (export_arrays[{index}] == NULL) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}",
                node.program.exports.len(),
                node.program.exports.len()
            )
            .unwrap();
        }
        for (import_index, import) in node.program.imports.iter().enumerate() {
            let Some((provider_index, export_index)) =
                graph_import_slot(graph, modules, &indices, node, import)?
            else {
                continue;
            };
            writeln!(
                output,
                "    import_arrays[{index}][{import_index}] = export_arrays[{provider_index}][{export_index}];"
            )
            .unwrap();
        }
        let import_count = node.program.imports.len();
        let export_count = node.program.exports.len();
        writeln!(
            output,
            "    status = {}(&proxy_host, &module_result, import_arrays[{index}], (size_t){import_count}, export_arrays[{index}], (size_t){export_count});\n    if (status != KILN_C99_OK) goto cleanup;",
            modules[index].1.symbol
        )
        .unwrap();
        if index == entry_index {
            output.push_str("    *result = module_result;\n");
        }
    }
    output.push_str("cleanup:\n    for (index = 0; index < (size_t)");
    write!(output, "{}; ++index) {{\n        free(import_arrays == NULL ? NULL : import_arrays[index]);\n        free(export_arrays == NULL ? NULL : export_arrays[index]);\n    }}\n    free(import_arrays);\n    free(export_arrays);\n    kiln_proxy_arena_clear(&arena);\n    return status;\n}}\n", modules.len()).unwrap();
    Ok(output)
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

fn decode_run_result(
    path: &Path,
    process_succeeded: bool,
) -> Result<Option<HostValue>, NativeError> {
    if !process_succeeded {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|error| NativeError::RunResult {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    decode_native_result(&bytes)
        .map(Some)
        .map_err(|error| NativeError::RunResult {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
}

fn materialize(path: &Path, bytes: &[u8], executable: bool) -> Result<(), NativeError> {
    let temporary = write_materialized_temporary(path, bytes, executable)?;
    let result = install_materialized(&temporary, path);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_materialized_temporary(
    path: &Path,
    bytes: &[u8],
    executable: bool,
) -> Result<PathBuf, NativeError> {
    write_materialized_temporary_with(path, bytes, executable, || {
        MATERIALIZE_COUNTER.fetch_add(1, Ordering::Relaxed)
    })
}

fn write_materialized_temporary_with(
    path: &Path,
    bytes: &[u8],
    executable: bool,
    mut next_counter: impl FnMut() -> u64,
) -> Result<PathBuf, NativeError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| NativeError::Io {
        operation: "create output directory",
        path: parent.to_path_buf(),
        source,
    })?;
    for _ in 0..128 {
        let temporary = materialized_temporary_path(path, next_counter());
        let mut file = match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(NativeError::Io {
                    operation: "create temporary artifact",
                    path: temporary,
                    source,
                });
            }
        };
        let result = (|| {
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
            Ok(temporary.clone())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    Err(NativeError::Io {
        operation: "create unique temporary artifact",
        path: path.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary artifact name collided 128 times",
        ),
    })
}

fn materialized_temporary_path(path: &Path, counter: u64) -> PathBuf {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    parent.join(format!(
        ".{}.{}-{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("kiln-artifact"),
        std::process::id(),
        counter,
    ))
}

fn publish_shared_artifacts(
    output: &Path,
    loader: &Path,
    header_source: &[u8],
    library: &[u8],
) -> Result<(), NativeError> {
    if output == loader {
        return Err(NativeError::ArtifactPathCollision {
            first: output.to_path_buf(),
            second: loader.to_path_buf(),
        });
    }
    // `File` locking semantics differ for separately opened handles in one process, so keep an
    // explicit process lock as well as the cross-process directory lock.
    let _process_lock = SHARED_PUBLICATION_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _directory_lock = acquire_shared_publication_lock(output)?;
    materialize_immutable(loader, library)?;
    // Publish the declaration first so replacing the library remains the final ready signal for
    // consumers. The directory lock keeps concurrent builders from interleaving two pairs;
    // readers still use the library replacement as the readiness boundary.
    materialize(&shared_library_header_path(output), header_source, false)?;
    materialize(output, library, false)
}

fn materialize_immutable(path: &Path, bytes: &[u8]) -> Result<(), NativeError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| NativeError::Io {
        operation: "create canonical shared-library directory",
        path: parent.to_path_buf(),
        source,
    })?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(|source| NativeError::Io {
        operation: "inspect canonical shared-library directory",
        path: parent.to_path_buf(),
        source,
    })?;
    if !parent_metadata.file_type().is_dir() {
        return Err(NativeError::ImmutableArtifactCollision {
            path: parent.to_path_buf(),
        });
    }
    if immutable_contents_match(path, bytes)? {
        return Ok(());
    }
    let temporary = write_materialized_temporary(path, bytes, false)?;
    let result = match fs::hard_link(&temporary, path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            immutable_contents_match(path, bytes).and_then(|matches| {
                matches
                    .then_some(())
                    .ok_or_else(|| NativeError::ImmutableArtifactCollision {
                        path: path.to_path_buf(),
                    })
            })
        }
        Err(source) => Err(NativeError::Io {
            operation: "install canonical shared library",
            path: path.to_path_buf(),
            source,
        }),
    };
    let cleanup = fs::remove_file(&temporary).map_err(|source| NativeError::Io {
        operation: "remove canonical shared-library temporary",
        path: temporary,
        source,
    });
    result.and(cleanup)
}

fn immutable_contents_match(path: &Path, bytes: &[u8]) -> Result<bool, NativeError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(NativeError::Io {
                operation: "inspect canonical shared library",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if !metadata.file_type().is_file() {
        return Err(NativeError::ImmutableArtifactCollision {
            path: path.to_path_buf(),
        });
    }
    let existing = fs::read(path).map_err(|source| NativeError::Io {
        operation: "read canonical shared library",
        path: path.to_path_buf(),
        source,
    })?;
    if existing == bytes {
        Ok(true)
    } else {
        Err(NativeError::ImmutableArtifactCollision {
            path: path.to_path_buf(),
        })
    }
}

fn acquire_shared_publication_lock(output: &Path) -> Result<fs::File, NativeError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| NativeError::Io {
        operation: "create shared publication directory",
        path: parent.to_path_buf(),
        source,
    })?;
    // Directory scope is intentional: `foo.so` and `foo.dylib` both derive `foo.h`, so locking
    // only the library pathname would still permit two writers to interleave the shared header.
    let lock = fs::File::open(parent).map_err(|source| NativeError::Io {
        operation: "open shared publication lock",
        path: parent.to_path_buf(),
        source,
    })?;
    lock.lock().map_err(|source| NativeError::Io {
        operation: "lock shared publication",
        path: parent.to_path_buf(),
        source,
    })?;
    Ok(lock)
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
    number_provider_a: PathBuf,
    executable: PathBuf,
}

struct SharedLibraryPaths {
    module_c: PathBuf,
    module_o: PathBuf,
    number_provider_a: PathBuf,
    library: PathBuf,
}

struct SharedGraphLibraryPaths {
    module_c: Vec<PathBuf>,
    module_o: Vec<PathBuf>,
    number_provider_a: PathBuf,
    wrapper_c: PathBuf,
    wrapper_o: PathBuf,
    library: PathBuf,
}

struct GraphBuildPaths {
    module_c: Vec<PathBuf>,
    module_o: Vec<PathBuf>,
    runtime_c: PathBuf,
    runtime_o: PathBuf,
    launcher_c: PathBuf,
    launcher_o: PathBuf,
    number_provider_a: PathBuf,
    executable: PathBuf,
}

impl GraphBuildPaths {
    fn new(root: &Path, module_count: usize) -> Self {
        let mut module_c = Vec::with_capacity(module_count);
        let mut module_o = Vec::with_capacity(module_count);
        for index in 0..module_count {
            module_c.push(root.join(format!("module-{index}.c")));
            module_o.push(root.join(format!("module-{index}.o")));
        }
        Self {
            module_c,
            module_o,
            runtime_c: root.join("runtime.c"),
            runtime_o: root.join("runtime.o"),
            launcher_c: root.join("launcher.c"),
            launcher_o: root.join("launcher.o"),
            number_provider_a: root.join("libkiln_native_number_provider.a"),
            executable: root.join(if cfg!(windows) {
                "program.exe"
            } else {
                "program"
            }),
        }
    }
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
            number_provider_a: root.join("libkiln_native_number_provider.a"),
            executable: root.join(if cfg!(windows) {
                "program.exe"
            } else {
                "program"
            }),
        }
    }
}

impl SharedLibraryPaths {
    fn new(root: &Path) -> Self {
        Self {
            module_c: root.join("module.c"),
            module_o: root.join("module.o"),
            number_provider_a: root.join("libkiln_native_number_provider.a"),
            library: root.join(shared_library_name()),
        }
    }
}

impl SharedGraphLibraryPaths {
    fn new(root: &Path, module_count: usize) -> Self {
        let mut module_c = Vec::with_capacity(module_count);
        let mut module_o = Vec::with_capacity(module_count);
        for index in 0..module_count {
            module_c.push(root.join(format!("module-{index}.c")));
            module_o.push(root.join(format!("module-{index}.o")));
        }
        Self {
            module_c,
            module_o,
            number_provider_a: root.join("libkiln_native_number_provider.a"),
            wrapper_c: root.join("wrapper.c"),
            wrapper_o: root.join("wrapper.o"),
            library: root.join(shared_library_name()),
        }
    }
}

#[cfg(target_os = "macos")]
const fn shared_library_name() -> &'static str {
    "program.dylib"
}

#[cfg(target_os = "linux")]
const fn shared_library_name() -> &'static str {
    "program.so"
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const fn shared_library_name() -> &'static str {
    "program.so"
}

/// Derive the public C header paired with a shared-library output.
///
/// This is intentionally a pure path transform so CLI and daemon preflight checks can protect
/// source files before invoking the native toolchain.
pub fn shared_library_header_path(output: &Path) -> PathBuf {
    output.with_extension("h")
}

fn shared_library_link_identity(output: &Path) -> String {
    let name = output
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new(shared_library_name()))
        .to_string_lossy();
    if cfg!(target_os = "macos") {
        format!("@rpath/{name}")
    } else {
        name.into_owned()
    }
}

#[allow(clippy::too_many_arguments)]
fn shared_library_content_identity(
    compiler: &str,
    backend: Backend,
    embedding_abi: &str,
    target: &str,
    profile: NativeProfile,
    public_symbol: &str,
    inputs: impl IntoIterator<Item = Digest>,
) -> Digest {
    let identity_options = Digest::hash(
        format!(
            "shared-loader-content-v1\0{}\0system-math\0{}\0{}",
            profile.as_str(),
            shared_library_link_policy(),
            public_symbol,
        )
        .as_bytes(),
    );
    key(
        ArtifactKind::LinkedArtifact,
        compiler,
        backend,
        embedding_abi,
        target,
        profile.cache(),
        identity_options,
        inputs,
    )
    .digest()
}

fn shared_library_loader_path(output: &Path, identity: Digest) -> Result<PathBuf, NativeError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| NativeError::Io {
        operation: "create shared output directory",
        path: parent.to_path_buf(),
        source,
    })?;
    let parent = fs::canonicalize(parent).map_err(|source| NativeError::Io {
        operation: "canonicalize shared output directory",
        path: parent.to_path_buf(),
        source,
    })?;
    const LOADER_DIRECTORY: &str = ".kiln-loaders";
    if output
        .file_name()
        .is_some_and(|name| name == LOADER_DIRECTORY)
        || parent
            .components()
            .any(|component| component.as_os_str() == LOADER_DIRECTORY)
    {
        return Err(NativeError::ArtifactPathCollision {
            first: output.to_path_buf(),
            second: parent,
        });
    }
    let extension = Path::new(shared_library_name())
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("so");
    Ok(parent.join(LOADER_DIRECTORY).join(format!(
        "kiln-{}.{}",
        identity.to_base64_url(),
        extension
    )))
}

const fn shared_library_link_policy() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos-content-loader-install-name-rpath-export-list-v3"
    } else if cfg!(target_os = "linux") {
        "linux-content-loader-soname-version-script-v3"
    } else {
        "unsupported-content-loader-shared-link-v2"
    }
}

fn configure_shared_library_link(
    options: &mut LinkOptions,
    output: &Path,
    public_symbol: &str,
    workspace: &Path,
) -> Result<(), NativeError> {
    let identity = shared_library_link_identity(output);
    #[cfg(target_os = "macos")]
    {
        let exports = workspace.join("exports.txt");
        materialize(&exports, format!("_{public_symbol}\n").as_bytes(), false)?;
        options
            .extra_args
            .push(format!("-Wl,-install_name,{identity}").into());
        options
            .extra_args
            .push(format!("-Wl,-exported_symbols_list,{}", exports.to_string_lossy()).into());
    }
    #[cfg(target_os = "linux")]
    {
        let exports = workspace.join("exports.map");
        let source = format!("KILN_1 {{\n  global:\n    {public_symbol};\n  local:\n    *;\n}};\n");
        materialize(&exports, source.as_bytes(), false)?;
        options
            .extra_args
            .push(format!("-Wl,-soname,{identity}").into());
        options
            .extra_args
            .push(format!("-Wl,--version-script,{}", exports.to_string_lossy()).into());
        options.extra_args.push("-Wl,--exclude-libs,ALL".into());
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = (options, identity, public_symbol, workspace);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_ir::{
        BYTECODE_VERSION, Constant, ExportBinding, HostOp, ImportBinding, Instruction,
        ModuleProgram,
    };
    use std::sync::{Arc, Barrier};

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new(prefix: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "{prefix}-{}-{}",
                std::process::id(),
                WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create native test directory");
            Self { path }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn environment_digest(kind: CompilerKind, entries: &[(&str, &str)]) -> Digest {
        let values = entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), std::ffi::OsString::from(value)))
            .collect::<std::collections::HashMap<_, _>>();
        compiler_environment_digest_with(kind, |name| values.get(name).cloned())
    }

    #[test]
    fn target_identity_framing_distinguishes_delimiters() {
        let first = target_identity_from_parts(
            "same-target",
            Some(std::ffi::OsStr::new("A;deployment=B")),
            None,
        );
        let second = target_identity_from_parts(
            "same-target",
            Some(std::ffi::OsStr::new("A")),
            Some(std::ffi::OsStr::new("B;deployment=")),
        );

        assert_ne!(first, second);
    }

    #[test]
    fn target_identity_framing_distinguishes_unset_from_empty() {
        let unset = target_identity_from_parts("same-target", None, None);
        let empty_sdk_root =
            target_identity_from_parts("same-target", Some(std::ffi::OsStr::new("")), None);
        let empty_deployment =
            target_identity_from_parts("same-target", None, Some(std::ffi::OsStr::new("")));

        assert_ne!(unset, empty_sdk_root);
        assert_ne!(unset, empty_deployment);
    }

    #[test]
    fn compiler_environment_identity_is_allowlisted_and_unambiguously_framed() {
        let clang_baseline = environment_digest(CompilerKind::Clang, &[]);
        for name in C_COMPILER_ENVIRONMENT {
            assert_ne!(
                environment_digest(CompilerKind::Clang, &[(name, "changed")]),
                clang_baseline,
                "{name} must participate in native cache identity"
            );
        }
        for name in GCC_COMPILER_ENVIRONMENT {
            assert_ne!(
                environment_digest(CompilerKind::Gcc, &[(name, "changed")]),
                environment_digest(CompilerKind::Gcc, &[]),
                "{name} must participate in GCC cache identity"
            );
        }
        for kind in [CompilerKind::Clang, CompilerKind::Zig] {
            assert_eq!(
                environment_digest(kind, &[("GCC_EXEC_PREFIX", "ignored")]),
                environment_digest(kind, &[]),
                "GCC-only policy must not invalidate {kind} artifacts"
            );
        }
        assert_ne!(
            environment_digest(CompilerKind::Zig, &[("ZIG_LIB_DIR", "changed")]),
            environment_digest(CompilerKind::Zig, &[]),
            "Zig's library override must participate in Zig cache identity"
        );
        assert_eq!(
            compiler_environment_digest_with(CompilerKind::Clang, |name| {
                (name == "KILN_UNRELATED").then(|| "ignored".into())
            }),
            clang_baseline,
            "variables outside the compiler allowlist do not affect artifacts"
        );
        for name in ["CPLUS_INCLUDE_PATH", "OBJC_INCLUDE_PATH"] {
            assert_eq!(
                environment_digest(CompilerKind::Clang, &[(name, "ignored")]),
                clang_baseline,
                "{name} does not affect a C11 translation unit"
            );
        }
        assert_ne!(
            environment_digest(CompilerKind::Clang, &[("CPATH", "")]),
            clang_baseline,
            "an empty search path is not the same compiler policy as an unset one"
        );
        assert_ne!(
            environment_digest(
                CompilerKind::Clang,
                &[("CPATH", "a"), ("C_INCLUDE_PATH", "bc")]
            ),
            environment_digest(
                CompilerKind::Clang,
                &[("CPATH", "ab"), ("C_INCLUDE_PATH", "c")]
            ),
            "length framing must distinguish adjacent values"
        );
    }

    #[test]
    fn compiler_environment_changes_miss_native_artifact_keys() {
        let root = TestDirectory::new("kiln-native-environment-key");
        let cache = ArtifactCache::new(root.path.join("cache"));
        let options = Digest::hash(b"same options");
        let inputs = [Digest::hash(b"same generated C")];
        let make_key = |environment| {
            key(
                ArtifactKind::NativeObject,
                compiler_identity_from_parts("same tool", environment),
                Backend::C,
                "same ABI",
                "same target",
                CacheProfile::Debug,
                options,
                inputs,
            )
        };
        let first = make_key(environment_digest(
            CompilerKind::Clang,
            &[("CPATH", "/first/include")],
        ));
        let second = make_key(environment_digest(
            CompilerKind::Clang,
            &[("CPATH", "/second/include")],
        ));

        cache.put(&first, b"object").expect("seed first identity");
        assert_eq!(
            cache.lookup(&first).expect("first lookup"),
            Lookup::Hit(kiln_artifact_cache::Artifact {
                bytes: b"object".to_vec(),
                digest: Digest::hash(b"object"),
                key_digest: first.digest(),
                layer_index: 0,
                path: cache.path_for(&first).expect("first path"),
            })
        );
        assert_eq!(cache.lookup(&second).expect("second lookup"), Lookup::Miss);
    }

    #[test]
    fn deterministic_payload_hits_reject_self_consistent_wrong_bytes() {
        let root = TestDirectory::new("kiln-native-expected-payload");
        let cache = ArtifactCache::new(root.path.join("cache"));
        let expected = b"deterministic generated C";
        let payload_key = key(
            ArtifactKind::GeneratedC,
            "kiln-test-generator",
            Backend::C,
            "test ABI",
            "test target",
            CacheProfile::Debug,
            Digest::hash(b"test options"),
            [Digest::hash(expected)],
        );
        cache
            .put(&payload_key, b"different but internally valid bytes")
            .expect("seed a self-consistent wrong record");

        let error = cached_payload(&cache, &payload_key, expected)
            .expect_err("a deterministic payload must match its expected bytes");
        assert!(
            matches!(
                error,
                NativeError::Cache(CacheError::Corrupt { ref reason, .. })
                    if reason.contains("does not match deterministic expected digest")
            ),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn successful_private_runs_require_one_well_formed_scalar_result() {
        let root = TestDirectory::new("kiln-native-result");
        let result_path = root.path.join("result.bin");

        assert!(matches!(
            decode_run_result(&result_path, true),
            Err(NativeError::RunResult { .. })
        ));
        assert_eq!(decode_run_result(&result_path, false).unwrap(), None);

        fs::write(&result_path, b"not-a-result").expect("write malformed result");
        assert!(matches!(
            decode_run_result(&result_path, true),
            Err(NativeError::RunResult { .. })
        ));

        let expected = HostValue::String("line one\nline two\0".to_owned());
        fs::write(
            &result_path,
            kiln_native_runtime::encode_native_result(&expected),
        )
        .expect("write valid result");
        assert_eq!(
            decode_run_result(&result_path, true).expect("decode valid result"),
            Some(expected)
        );
    }

    #[test]
    fn shared_publication_serializes_each_header_library_pair() {
        let root = TestDirectory::new("kiln-native-shared-publication");
        let output = root.path.join("program.so");
        let loader = root.path.join("kiln-content.so");
        let header = shared_library_header_path(&output);
        materialize(&header, b"old header", false).expect("publish old header");
        materialize(&output, b"old library", false).expect("publish old library");

        let process_lock = SHARED_PUBLICATION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let writer_output = output.clone();
        let writer_loader = loader.clone();
        let writer = std::thread::spawn(move || {
            entered_tx.send(()).expect("announce waiting writer");
            let result = publish_shared_artifacts(
                &writer_output,
                &writer_loader,
                b"new header",
                b"new library",
            );
            finished_tx.send(result).expect("report publication");
        });
        entered_rx.recv().expect("writer starts");
        assert!(
            finished_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "a second publisher must wait for the pair lock"
        );
        assert_eq!(fs::read(&header).expect("read old header"), b"old header");
        assert_eq!(fs::read(&output).expect("read old library"), b"old library");

        drop(process_lock);
        finished_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("writer completes after unlock")
            .expect("publication succeeds");
        writer.join().expect("writer exits");
        assert_eq!(fs::read(&header).expect("read new header"), b"new header");
        assert_eq!(fs::read(&output).expect("read new library"), b"new library");
        assert_eq!(
            fs::read(&loader).expect("read loader library"),
            b"new library"
        );

        let collision =
            publish_shared_artifacts(&output, &loader, b"other header", b"other library")
                .expect_err("a canonical content name is immutable");
        assert!(matches!(
            collision,
            NativeError::ImmutableArtifactCollision { .. }
        ));
        assert_eq!(fs::read(header).unwrap(), b"new header");
        assert_eq!(fs::read(&output).unwrap(), b"new library");

        #[cfg(unix)]
        {
            fs::remove_file(&loader).expect("remove regular loader");
            std::os::unix::fs::symlink(&output, &loader).expect("create matching loader symlink");
            let error = publish_shared_artifacts(&output, &loader, b"new header", b"new library")
                .expect_err("a symlink cannot stand in for an immutable loader");
            assert!(matches!(
                error,
                NativeError::ImmutableArtifactCollision { .. }
            ));
            assert!(
                fs::symlink_metadata(&loader)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
    }

    #[test]
    fn materialization_skips_a_stale_temporary_name() {
        let root = TestDirectory::new("kiln-native-stale-temporary");
        let destination = root.path.join("program");
        let stale = materialized_temporary_path(&destination, 7);
        fs::write(&stale, b"crashed writer").expect("seed stale temporary");
        let mut counters = [7, 8].into_iter();

        let temporary =
            write_materialized_temporary_with(&destination, b"new artifact", false, || {
                counters.next().expect("bounded retry sequence")
            })
            .expect("stale temporary is skipped");

        assert_eq!(temporary, materialized_temporary_path(&destination, 8));
        assert_eq!(
            fs::read(&stale).expect("stale file remains"),
            b"crashed writer"
        );
        assert_eq!(
            fs::read(&temporary).expect("read new temporary"),
            b"new artifact"
        );
        fs::remove_file(temporary).expect("remove new temporary");
    }

    #[test]
    fn immutable_materialization_never_replaces_a_concurrent_winner() {
        let root = TestDirectory::new("kiln-native-immutable-race");
        let destination = root.path.join("content.so");
        let barrier = Arc::new(Barrier::new(3));
        let writers = [b"first payload".as_slice(), b"second payload".as_slice()]
            .into_iter()
            .map(|payload| {
                let destination = destination.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    materialize_immutable(&destination, payload)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = writers
            .into_iter()
            .map(|writer| writer.join().expect("immutable writer exits"))
            .collect::<Vec<_>>();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(NativeError::ImmutableArtifactCollision { .. })
                ))
                .count(),
            1
        );
        let installed = fs::read(destination).expect("read concurrent winner");
        assert!(
            installed == b"first payload" || installed == b"second payload",
            "unexpected installed payload: {installed:?}"
        );
    }

    #[test]
    fn shared_loader_paths_canonicalize_and_reserve_the_loader_namespace() {
        let root = TestDirectory::new("kiln-native-loader-path");
        let real = root.path.join("real");
        fs::create_dir(&real).expect("create real output directory");
        let identity = Digest::hash(b"loader-path-test");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, root.path.join("alias"))
                .expect("create output-directory symlink");
            let loader = shared_library_loader_path(&root.path.join("alias/program.so"), identity)
                .expect("derive loader through symlink");
            let canonical_real = fs::canonicalize(&real).expect("canonical real directory");
            assert_eq!(
                loader.parent(),
                Some(canonical_real.join(".kiln-loaders").as_path())
            );
        }

        let reserved = real.join(".kiln-loaders");
        fs::create_dir(&reserved).expect("create reserved loader directory");
        assert!(matches!(
            shared_library_loader_path(&reserved.join("program.so"), identity),
            Err(NativeError::ArtifactPathCollision { .. })
        ));
        let nested = reserved.join("nested");
        fs::create_dir(&nested).expect("create nested reserved directory");
        assert!(matches!(
            shared_library_loader_path(&nested.join("program.so"), identity),
            Err(NativeError::ArtifactPathCollision { .. })
        ));
        assert!(matches!(
            shared_library_loader_path(&real.join(".kiln-loaders"), identity),
            Err(NativeError::ArtifactPathCollision { .. })
        ));
    }

    fn one_module_graph(project: &Path, value: f64, export: &str) -> ModuleGraph {
        fs::create_dir_all(project).expect("create one-module graph project");
        fs::write(
            project.join("main.js"),
            format!("export const {export} = {value};\n"),
        )
        .expect("write one-module graph entry");
        let module = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                constants: vec![Constant::Number(value.to_bits())],
                code: vec![
                    Instruction::Const(0),
                    Instruction::StoreLocal(0),
                    Instruction::LoadLocal(0),
                    Instruction::Return,
                ],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: vec![ExportBinding {
                exported: export.to_owned(),
                local_slot: 0,
            }],
        };
        ModuleGraph::build_with_modules(project.join("main.js"), project, move |_| module.clone())
            .expect("construct one-module graph")
    }

    fn star_reexport_graph(project: &Path) -> ModuleGraph {
        fs::create_dir_all(project).expect("create star graph project");
        for name in ["origin.js", "barrel.js", "main.js"] {
            fs::write(project.join(name), "").expect("write star graph module");
        }
        let origin = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                constants: vec![Constant::Number(42.0_f64.to_bits())],
                code: vec![
                    Instruction::Const(0),
                    Instruction::StoreLocal(0),
                    Instruction::Const(0),
                    Instruction::Return,
                ],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: vec![ExportBinding {
                exported: "answer".into(),
                local_slot: 0,
            }],
        };
        let barrel = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                constants: vec![Constant::Undefined],
                code: vec![Instruction::Const(0), Instruction::Return],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: vec!["./origin.js".into()],
            imports: vec![ImportBinding {
                source: "./origin.js".into(),
                imported: kiln_ir::NAMESPACE_IMPORT.into(),
                local: "*reexport0*".into(),
                local_slot: 0,
            }],
            exports: vec![ExportBinding {
                exported: format!("{}./origin.js", kiln_ir::STAR_REEXPORT_PREFIX),
                local_slot: 0,
            }],
        };
        let entry = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                constants: vec![Constant::Undefined],
                code: vec![
                    Instruction::LoadLocal(0),
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 1,
                    },
                    Instruction::Pop,
                    Instruction::Const(0),
                    Instruction::Return,
                ],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: vec!["./barrel.js".into()],
            imports: vec![ImportBinding {
                source: "./barrel.js".into(),
                imported: "answer".into(),
                local: "answer".into(),
                local_slot: 0,
            }],
            exports: Vec::new(),
        };
        let origin_path = fs::canonicalize(project.join("origin.js")).expect("canonical origin");
        let barrel_path = fs::canonicalize(project.join("barrel.js")).expect("canonical barrel");
        ModuleGraph::build_with_modules(project.join("main.js"), project, move |path| {
            if path == origin_path {
                origin.clone()
            } else if path == barrel_path {
                barrel.clone()
            } else {
                entry.clone()
            }
        })
        .expect("construct star re-export graph")
    }

    fn namespace_import_graph(project: &Path) -> ModuleGraph {
        fs::create_dir_all(project).expect("create namespace graph project");
        for name in ["origin.js", "main.js"] {
            fs::write(project.join(name), "").expect("write namespace graph module");
        }
        let origin = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                constants: vec![Constant::Number(42.0_f64.to_bits())],
                code: vec![
                    Instruction::Const(0),
                    Instruction::StoreLocal(0),
                    Instruction::Const(0),
                    Instruction::Return,
                ],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: vec![ExportBinding {
                exported: "answer".into(),
                local_slot: 0,
            }],
        };
        let entry = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                constants: vec![Constant::Undefined],
                code: vec![Instruction::Const(0), Instruction::Return],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: vec!["./origin.js".into()],
            imports: vec![ImportBinding {
                source: "./origin.js".into(),
                imported: kiln_ir::NAMESPACE_IMPORT.into(),
                local: "namespace".into(),
                local_slot: 0,
            }],
            exports: Vec::new(),
        };
        let origin_path = fs::canonicalize(project.join("origin.js")).expect("canonical origin");
        ModuleGraph::build_with_modules(project.join("main.js"), project, move |path| {
            if path == origin_path {
                origin.clone()
            } else {
                entry.clone()
            }
        })
        .expect("construct namespace import graph")
    }

    #[test]
    fn native_graph_wiring_uses_resolved_star_export_origins() {
        let root = TestDirectory::new("kiln-native-star-graph");
        let graph = star_reexport_graph(&root.path.join("project"));
        let modules = graph
            .modules()
            .map(|node| {
                (
                    node,
                    kiln_backend_c::emit_module(&node.program).expect("module emits"),
                )
            })
            .collect::<Vec<_>>();

        let launcher = graph_launcher_source(&graph, &modules).expect("launcher emits");
        assert!(launcher.contains("kiln_imports_2[0] = kiln_exports_0[0];"));
        assert!(!launcher.contains("kiln_imports_1[0] ="));
        let wrapper = graph_shared_wrapper_source(&graph, &modules, "kiln_star_graph")
            .expect("shared wrapper emits");
        assert!(wrapper.contains("import_arrays[2][0] = export_arrays[0][0];"));
        assert!(!wrapper.contains("import_arrays[1][0] ="));
        assert!(wrapper.contains("import_arrays[1] = (kiln_value *)calloc"));

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native star integration: no C compiler found");
            return;
        };
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        let run = builder
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("program"),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("native star graph runs");
        assert_eq!(run.stdout, b"42\n");
        builder
            .build_shared_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join(shared_library_name()),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("native shared star graph links");
    }

    #[test]
    fn native_graphs_reject_user_visible_namespace_values_explicitly() {
        let root = TestDirectory::new("kiln-native-namespace-graph");
        let graph = namespace_import_graph(&root.path.join("project"));
        let modules = graph
            .modules()
            .map(|node| {
                (
                    node,
                    kiln_backend_c::emit_module(&node.program).expect("module emits"),
                )
            })
            .collect::<Vec<_>>();

        assert!(matches!(
            graph_launcher_source(&graph, &modules),
            Err(NativeError::Emit(EmitError::Unsupported { feature }))
                if feature.contains("namespace imports")
        ));
        assert!(matches!(
            graph_shared_wrapper_source(&graph, &modules, "kiln_namespace_graph"),
            Err(NativeError::Emit(EmitError::Unsupported { feature }))
                if feature.contains("namespace imports")
        ));
    }

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
            functions: Vec::new(),
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
    fn embedded_shared_library_calls_host_without_cli_runtime_and_reuses_cache() {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            eprintln!("skipping embedded native integration on unsupported platform");
            return;
        }
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping embedded native integration: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-embedded-test");
        let cache = root.path.join("cache");
        let library_path =
            root.path
                .join("embedded")
                .with_extension(if cfg!(target_os = "macos") {
                    "dylib"
                } else {
                    "so"
                });
        let builder = NativeBuilder::new(ArtifactCache::new(cache), toolchain.clone());
        let program = Program {
            version: BYTECODE_VERSION,
            constants: vec![
                Constant::String("embedded\0tail=".to_owned()),
                Constant::Number(0.1_f64.to_bits()),
            ],
            code: vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::Binary(kiln_ir::BinaryOp::Add),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };
        let colliding_output = root.path.join("embedded-header.H");
        let collision = builder
            .build_shared_library(
                &program,
                &NativeBuildOptions {
                    output: colliding_output.clone(),
                    profile: NativeProfile::Dev,
                },
            )
            .expect_err("a library output cannot alias its derived header");
        assert!(matches!(
            collision,
            NativeError::ArtifactPathCollision { .. }
        ));
        assert!(!colliding_output.exists());
        let options = NativeBuildOptions {
            output: library_path.clone(),
            profile: NativeProfile::Dev,
        };
        let first = builder
            .build_shared_library(&program, &options)
            .expect("first embedded shared-library build succeeds");
        assert!(!first.cache_hits.generated_c);
        assert!(!first.cache_hits.object);
        assert!(!first.cache_hits.library);
        let executable = builder
            .build(
                &program,
                &NativeBuildOptions {
                    output: root.path.join("ordinary-program"),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("ordinary executable build succeeds after PIC build");
        assert!(executable.cache_hits.generated_c);
        assert!(
            !executable.cache_hits.application_object,
            "a shared PIC object must not satisfy an executable-object lookup"
        );
        let second = builder
            .build_shared_library(&program, &options)
            .expect("second embedded shared-library build succeeds");
        assert!(second.cache_hits.generated_c);
        assert!(second.cache_hits.object);
        assert!(second.cache_hits.library);
        assert_eq!(first.module_symbol, second.module_symbol);
        assert_eq!(second.output, library_path);
        assert!(second.loader.is_file());
        assert_eq!(
            fs::read(&second.output).unwrap(),
            fs::read(&second.loader).unwrap()
        );
        assert_eq!(second.loader, first.loader);
        assert_eq!(first.header, library_path.with_extension("h"));
        assert_eq!(second.header, first.header);
        let published_header = fs::read_to_string(&second.header).expect("read embedding header");
        assert!(published_header.contains(&format!("int {}(", second.module_symbol)));

        let renamed_path = root
            .path
            .join("renamed")
            .with_extension(if cfg!(target_os = "macos") {
                "dylib"
            } else {
                "so"
            });
        let renamed = builder
            .build_shared_library(
                &program,
                &NativeBuildOptions {
                    output: renamed_path,
                    profile: NativeProfile::Dev,
                },
            )
            .expect("renamed shared-library build succeeds");
        assert!(renamed.cache_hits.generated_c);
        assert!(renamed.cache_hits.object);
        assert!(
            renamed.cache_hits.library,
            "a content-named loader is reusable across requested output aliases"
        );
        assert_eq!(renamed.loader, first.loader);
        assert_eq!(renamed.header, renamed.output.with_extension("h"));

        let host_source = root.path.join("host.c");
        let host_object = root.path.join("host.o");
        let host_executable = root.path.join("host");
        let mut source = String::from("#include \"");
        source.push_str(
            second
                .header
                .file_name()
                .and_then(|name| name.to_str())
                .expect("embedding header has a UTF-8 file name"),
        );
        source.push_str(
            r#""
#include <stdio.h>
#include <string.h>

struct capture {
    char bytes[256];
    size_t length;
    int calls;
};

"#,
        );
        source.push_str(
            r#"static int capture_host_call(void *context, uint16_t operation, uint16_t argc,
                             const kiln_value *arguments, kiln_value *result) {
    struct capture *capture = (struct capture *)context;
    if (capture == NULL || result == NULL || operation != KILN_HOST_OP_WRITE_STDOUT
        || argc != 1 || arguments == NULL || arguments[0].tag != KILN_VALUE_STRING
        || arguments[0].ptr == NULL || arguments[0].len >= sizeof(capture->bytes)) {
        return KILN_C99_HOST_ERROR;
    }
    memcpy(capture->bytes, arguments[0].ptr, arguments[0].len);
    capture->length = arguments[0].len;
    capture->bytes[capture->length++] = '\n';
    capture->calls += 1;
    *result = (kiln_value){KILN_VALUE_UNDEFINED, 0, 0, NULL, 0};
    return KILN_C99_OK;
}

int main(void) {
    struct capture capture = {{0}, 0, 0};
    kiln_host host = {capture_host_call, &capture};
    kiln_value result;
    int status = "#,
        );
        source.push_str(&second.module_symbol);
        source.push_str(
            r#"(&host, &result);
    if (status != KILN_C99_OK || capture.calls != 1) {
        return 2;
    }
    if (fwrite(capture.bytes, 1, capture.length, stdout) != capture.length) {
        return 3;
    }
    return 0;
}
"#,
        );
        fs::write(&host_source, source).expect("write embedding host");
        let mut compile_options = CompileOptions::default();
        compile_options.include_dirs.push(root.path.clone());
        toolchain
            .compile_c(&host_source, &host_object, &compile_options)
            .expect("compile embedding host");
        let mut link_options = LinkOptions::default();
        link_options.runtime_libraries.push(library_path.clone());
        let loader_directory = second.loader.parent().expect("loader has directory");
        link_options
            .extra_args
            .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
        toolchain
            .link_executable(
                std::slice::from_ref(&host_object),
                &host_executable,
                &link_options,
            )
            .expect("link embedding host");
        let mut command = Command::new(&host_executable);
        #[cfg(target_os = "linux")]
        command.env("LD_LIBRARY_PATH", loader_directory);
        #[cfg(target_os = "macos")]
        command.env("DYLD_LIBRARY_PATH", loader_directory);
        let output = command.output().expect("run embedding host");
        assert!(
            output.status.success(),
            "embedding host failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"embedded\0tail=0.1\n");
        assert!(output.stderr.is_empty());
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
        let programs = [stdout_program("alpha", 1.0), stdout_program("beta", 2.0)];
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
            }));
        }
        for (index, thread) in threads.into_iter().enumerate() {
            let expected: &[u8] = if index % 2 == 0 {
                b"alpha\n"
            } else {
                b"beta\n"
            };
            let report = thread.join().expect("run thread does not panic");
            assert_eq!(report.stdout, expected);
            let expected_result = if index % 2 == 0 { 1.0_f64 } else { 2.0_f64 };
            assert_eq!(
                report.result,
                Some(HostValue::Number(expected_result.to_bits()))
            );
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
            let interpreted = kiln_interpreter::execute(
                std::rc::Rc::new(program.clone()),
                &mut host,
                Default::default(),
            )
            .unwrap_or_else(|error| panic!("interpreter fixture {name} failed: {error}"));
            let expected = host.into_stdout().into_bytes();
            let expected_result = HostValue::from(interpreted.value);
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
            assert_eq!(native.result, Some(expected_result), "fixture {name}");
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn graph_build_reuses_sibling_objects_across_different_roots() {
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native graph integration: no C compiler found");
            return;
        };
        let root = std::env::temp_dir().join(format!(
            "kiln-native-graph-test-{}-{}",
            std::process::id(),
            WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let first_project = root.join("first");
        let second_project = root.join("second");
        for project in [&first_project, &second_project] {
            fs::create_dir_all(project).expect("create graph project");
            fs::write(project.join("main.js"), "").expect("write graph entry");
            fs::write(project.join("dep.js"), "").expect("write graph dependency");
        }
        let dependency = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                constants: vec![Constant::Number(42.0_f64.to_bits())],
                code: vec![
                    Instruction::Const(0),
                    Instruction::StoreLocal(0),
                    Instruction::Const(0),
                    Instruction::Return,
                ],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: vec![ExportBinding {
                exported: "value".into(),
                local_slot: 0,
            }],
        };
        let entry = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                constants: vec![Constant::Number(99.0_f64.to_bits())],
                code: vec![
                    Instruction::LoadLocal(0),
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 1,
                    },
                    Instruction::Pop,
                    Instruction::Const(0),
                    Instruction::Return,
                ],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: vec!["./dep.js".into()],
            imports: vec![ImportBinding {
                source: "./dep.js".into(),
                imported: "value".into(),
                local: "value".into(),
                local_slot: 0,
            }],
            exports: Vec::new(),
        };
        let make_graph = |project: &Path| {
            let entry_path = project.join("main.js");
            let dependency_path =
                fs::canonicalize(project.join("dep.js")).expect("canonical dependency");
            let dependency = dependency.clone();
            let entry = entry.clone();
            ModuleGraph::build_with_modules(entry_path, project, move |path| {
                if path == dependency_path {
                    dependency.clone()
                } else {
                    entry.clone()
                }
            })
            .expect("construct graph")
        };
        let first_graph = make_graph(&first_project);
        let second_graph = make_graph(&second_project);
        let first_modules = first_graph
            .modules()
            .map(|node| {
                (
                    node,
                    kiln_backend_c::emit_module(&node.program).expect("module emits"),
                )
            })
            .collect::<Vec<_>>();
        let launcher = graph_launcher_source(&first_graph, &first_modules).expect("launcher emits");
        let main = launcher.find("int main(void)").expect("launcher has main");
        assert!(launcher[..main].contains("static kiln_value kiln_exports_"));
        assert!(launcher[..main].contains("static kiln_value kiln_imports_"));
        assert!(!launcher[main..].contains("kiln_value kiln_exports_"));
        assert!(!launcher[main..].contains("kiln_value kiln_imports_"));
        let builder = NativeBuilder::new(ArtifactCache::new(root.join("cache")), toolchain);
        let first = builder
            .build_graph(
                &first_graph,
                &NativeBuildOptions {
                    output: root.join("first-program"),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("first graph build succeeds");
        assert!(!first.cache_hits.application_object);
        let second = builder
            .run_graph(
                &second_graph,
                &NativeBuildOptions {
                    output: root.join("second-program"),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("second graph build succeeds");
        assert!(second.build.cache_hits.generated_c);
        assert!(second.build.cache_hits.application_object);
        assert!(second.build.cache_hits.launcher_object);
        assert!(second.build.cache_hits.executable);
        assert_eq!(second.stdout, b"42\n");
        assert_eq!(second.result, Some(HostValue::Number(99.0_f64.to_bits())));

        assert_eq!(
            second
                .build
                .module_artifacts
                .iter()
                .map(|module| module.logical_path.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("dep.js"), Some("main.js")]
        );
        assert!(second.build.runtime_generated_c_cache_hit);
        assert!(second.build.launcher_generated_c_cache_hit);

        let edited_dependency = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                constants: vec![Constant::Number(43.0_f64.to_bits())],
                code: vec![
                    Instruction::Const(0),
                    Instruction::StoreLocal(0),
                    Instruction::Const(0),
                    Instruction::Return,
                ],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: vec![ExportBinding {
                exported: "value".into(),
                local_slot: 0,
            }],
        };
        let edited_graph = {
            let entry_path = first_project.join("main.js");
            let dependency_path =
                fs::canonicalize(first_project.join("dep.js")).expect("canonical dependency");
            let edited_dependency = edited_dependency.clone();
            let entry = entry.clone();
            ModuleGraph::build_with_modules(entry_path, &first_project, move |path| {
                if path == dependency_path {
                    edited_dependency.clone()
                } else {
                    entry.clone()
                }
            })
            .expect("construct edited graph")
        };
        let edited = builder
            .build_graph(
                &edited_graph,
                &NativeBuildOptions {
                    output: root.join("edited-program"),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("edited graph build succeeds");
        let generated_c_misses = edited
            .module_artifacts
            .iter()
            .filter(|module| !module.generated_c_cache_hit)
            .count();
        let object_misses = edited
            .module_artifacts
            .iter()
            .filter(|module| !module.object_cache_hit)
            .count();
        assert_eq!(generated_c_misses, 1);
        assert_eq!(object_misses, 1);
        assert_eq!(
            edited.module_artifacts[0].logical_path.as_deref(),
            Some("dep.js")
        );
        assert!(!edited.module_artifacts[0].generated_c_cache_hit);
        assert!(!edited.module_artifacts[0].object_cache_hit);
        assert!(edited.module_artifacts[1].generated_c_cache_hit);
        assert!(edited.module_artifacts[1].object_cache_hit);
        assert!(edited.cache_hits.runtime_object);
        assert!(edited.cache_hits.launcher_object);
        assert!(edited.runtime_generated_c_cache_hit);
        assert!(edited.launcher_generated_c_cache_hit);
        assert!(!edited.cache_hits.executable);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn shared_graph_wrapper_emits_and_links_without_static_value_arrays() {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            eprintln!("skipping native shared graph integration on unsupported platform");
            return;
        }
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native shared graph integration: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-shared-graph-test");
        let project = root.path.join("project");
        fs::create_dir_all(&project).expect("create shared graph project");
        fs::write(project.join("main.js"), "").expect("write shared graph entry");
        fs::write(project.join("dep.js"), "").expect("write shared graph dependency");
        let dependency = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                constants: vec![Constant::Number(7.0_f64.to_bits())],
                code: vec![
                    Instruction::Const(0),
                    Instruction::StoreLocal(0),
                    Instruction::Const(0),
                    Instruction::Return,
                ],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: vec![ExportBinding {
                exported: "value".into(),
                local_slot: 0,
            }],
        };
        let entry = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                constants: Vec::new(),
                code: vec![Instruction::LoadLocal(0), Instruction::Return],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: vec!["./dep.js".into()],
            imports: vec![ImportBinding {
                source: "./dep.js".into(),
                imported: "value".into(),
                local: "value".into(),
                local_slot: 0,
            }],
            exports: Vec::new(),
        };
        let dependency_path = fs::canonicalize(project.join("dep.js")).expect("canonical dep");
        let graph =
            ModuleGraph::build_with_modules(project.join("main.js"), &project, move |path| {
                if path == dependency_path {
                    dependency.clone()
                } else {
                    entry.clone()
                }
            })
            .expect("construct shared graph");
        let modules = graph
            .modules()
            .map(|node| {
                (
                    node,
                    emit_module_with_logical_path(
                        &node.program,
                        &logical_module_path(graph.project_root(), &node.path)
                            .expect("logical module path"),
                    )
                    .expect("module emits"),
                )
            })
            .collect::<Vec<_>>();
        let wrapper_symbol = format!(
            "kiln_shared_graph_{}",
            graph_shared_topology_digest(&graph, &modules).to_hex()
        );
        let wrapper =
            graph_shared_wrapper_source(&graph, &modules, &wrapper_symbol).expect("wrapper emits");
        assert!(wrapper.contains(embedding_header()));
        assert!(wrapper.contains("extern int kiln_module_path_"));
        assert!(wrapper.contains("malloc((size_t)"));
        assert!(!wrapper.contains("static kiln_value kiln_import"));
        assert!(!wrapper.contains("static kiln_value kiln_export"));
        assert!(!wrapper.contains("kiln_cli_host_call"));
        assert!(!wrapper.contains("setlocale"));
        let builder = NativeBuilder::new(
            ArtifactCache::new(root.path.join("cache")),
            toolchain.clone(),
        );
        let report = builder
            .build_shared_graph(
                &graph,
                &NativeBuildOptions {
                    output: root
                        .path
                        .join("built")
                        .with_extension(if cfg!(target_os = "macos") {
                            "dylib"
                        } else {
                            "so"
                        }),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("build shared graph succeeds");
        assert_eq!(report.module_symbol, wrapper_symbol);
        assert!(report.output.is_file());
        let cached = builder
            .build_shared_graph(
                &graph,
                &NativeBuildOptions {
                    output: report.output.clone(),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("cached shared graph succeeds");
        assert!(cached.cache_hits.generated_c);
        assert!(cached.cache_hits.object);
        assert!(cached.cache_hits.library);
        assert_eq!(cached.loader, report.loader);
        assert!(cached.loader.is_file());
        let renamed = builder
            .build_shared_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("renamed").with_extension(
                        if cfg!(target_os = "macos") {
                            "dylib"
                        } else {
                            "so"
                        },
                    ),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("renamed shared graph succeeds");
        assert!(renamed.cache_hits.generated_c);
        assert!(renamed.cache_hits.object);
        assert!(renamed.cache_hits.library);
        assert_eq!(renamed.loader, report.loader);
        let wrapper_source = root.path.join("wrapper.c");
        fs::write(&wrapper_source, wrapper).expect("write wrapper source");
        let compile_options = CompileOptions {
            position_independent: true,
            ..CompileOptions::default()
        };
        let mut objects = Vec::with_capacity(modules.len() + 1);
        for (index, (_, module)) in modules.iter().enumerate() {
            let source = root.path.join(format!("module-{index}.c"));
            let object = root.path.join(format!("module-{index}.o"));
            fs::write(&source, &module.source).expect("write module source");
            toolchain
                .compile_c(&source, &object, &compile_options)
                .expect("compile module source");
            objects.push(object);
        }
        let wrapper_object = root.path.join("wrapper.o");
        toolchain
            .compile_c(&wrapper_source, &wrapper_object, &compile_options)
            .expect("compile wrapper source");
        objects.push(wrapper_object);
        let library = root.path.join(shared_library_name());
        toolchain
            .link_shared_library(&objects, &library, &LinkOptions::default())
            .expect("link shared graph");
        assert!(library.is_file());
        let header = root.path.join("wrapper.h");
        fs::write(
            &header,
            graph_shared_header(&wrapper_symbol).expect("wrapper header emits"),
        )
        .expect("write wrapper header");
        let host_source = root.path.join("null-host.c");
        fs::write(
            &host_source,
            format!(
                "#include \"wrapper.h\"\nint main(void) {{ kiln_value result; return {}(NULL, &result) != KILN_C99_OK || result.tag != KILN_VALUE_NUMBER; }}\n",
                wrapper_symbol
            ),
        )
        .expect("write null-host source");
        let host_object = root.path.join("null-host.o");
        toolchain
            .compile_c(&host_source, &host_object, &compile_options)
            .expect("compile null-host source");
        let host_executable = root.path.join("null-host");
        let mut host_link = LinkOptions::default();
        host_link.runtime_libraries.push(library.clone());
        host_link
            .extra_args
            .push(format!("-Wl,-rpath,{}", root.path.display()).into());
        toolchain
            .link_executable(&[host_object], &host_executable, &host_link)
            .expect("link null-host executable");
        let output = Command::new(&host_executable)
            .output()
            .expect("run null-host executable");
        assert!(
            output.status.success(),
            "null-host graph failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn shared_graph_proxy_preserves_host_strings_across_module_boundaries() {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            eprintln!("skipping native shared graph host integration on unsupported platform");
            return;
        }
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native shared graph host integration: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-shared-graph-host-test");
        let project = root.path.join("project");
        fs::create_dir_all(&project).expect("create shared graph host project");
        fs::write(project.join("main.js"), "").expect("write shared graph host entry");
        fs::write(project.join("dep.js"), "").expect("write shared graph host dependency");

        let dependency = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                constants: Vec::new(),
                code: vec![
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 0,
                    },
                    Instruction::StoreLocal(0),
                    Instruction::LoadLocal(0),
                    Instruction::Return,
                ],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: vec![ExportBinding {
                exported: "value".into(),
                local_slot: 0,
            }],
        };
        let entry = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                constants: Vec::new(),
                code: vec![
                    Instruction::LoadLocal(0),
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 1,
                    },
                    Instruction::Return,
                ],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: vec!["./dep.js".into()],
            imports: vec![ImportBinding {
                source: "./dep.js".into(),
                imported: "value".into(),
                local: "value".into(),
                local_slot: 0,
            }],
            exports: Vec::new(),
        };
        let dependency_path = fs::canonicalize(project.join("dep.js")).expect("canonical dep");
        let graph =
            ModuleGraph::build_with_modules(project.join("main.js"), &project, move |path| {
                if path == dependency_path {
                    dependency.clone()
                } else {
                    entry.clone()
                }
            })
            .expect("construct shared host graph");
        let builder = NativeBuilder::new(
            ArtifactCache::new(root.path.join("cache")),
            toolchain.clone(),
        );
        let report = builder
            .build_shared_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join(shared_library_name()),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("build shared host graph");

        let host_source = root.path.join("host.c");
        let host_object = root.path.join("host.o");
        let host_executable = root.path.join("host");
        let header_name = report
            .header
            .file_name()
            .and_then(|name| name.to_str())
            .expect("shared graph header file name");
        fs::write(
            &host_source,
            format!(
                r#"#include "{header_name}"
#include <string.h>

typedef struct host_context {{
    int mode;
    char storage[4];
}} host_context;

static int host_call(void *opaque, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    host_context *context = (host_context *)opaque;
    if (context == NULL || operation != KILN_HOST_OP_WRITE_STDOUT || result == NULL)
        return KILN_C99_HOST_ERROR;
    if (argc == 0) {{
        if (context->mode == 2) return 7;
        if (context->mode == 3) {{
            *result = (kiln_value){{KILN_VALUE_STRING, 0, 0, NULL, 3}};
            return KILN_C99_OK;
        }}
        if (context->mode == 1) {{
            *result = (kiln_value){{KILN_VALUE_STRING, 0, 0, NULL, 0}};
            return KILN_C99_OK;
        }}
        memcpy(context->storage, "dep", 3);
        *result = (kiln_value){{KILN_VALUE_STRING, 0, 0, context->storage, 3}};
        return KILN_C99_OK;
    }}
    if (argc != 1 || arguments == NULL || arguments[0].tag != KILN_VALUE_STRING)
        return KILN_C99_HOST_ERROR;
    if (context->mode == 1) {{
        if (arguments[0].len != 0 || arguments[0].ptr == NULL)
            return KILN_C99_HOST_ERROR;
    }} else if (arguments[0].len != 3 || arguments[0].ptr == NULL ||
               arguments[0].ptr == context->storage ||
               memcmp(arguments[0].ptr, "dep", 3) != 0) {{
        return KILN_C99_HOST_ERROR;
    }}
    *result = (kiln_value){{KILN_VALUE_UNDEFINED, 0, 0, NULL, 0}};
    return KILN_C99_OK;
}}

int main(void) {{
    host_context context = {{0, {{0}}}};
    kiln_host host = {{host_call, &context}};
    kiln_value result;
    int index;
    for (index = 0; index < 64; ++index) {{
        if ({symbol}(&host, &result) != KILN_C99_OK) return 2;
    }}
    context.mode = 1;
    if ({symbol}(&host, &result) != KILN_C99_OK) return 3;
    context.mode = 2;
    if ({symbol}(&host, &result) != KILN_C99_HOST_ERROR) return 4;
    context.mode = 3;
    if ({symbol}(&host, &result) != KILN_C99_HOST_ERROR) return 5;
    if ({symbol}(NULL, &result) != KILN_C99_HOST_ERROR) return 6;
    return 0;
}}
"#,
                symbol = report.module_symbol,
            ),
        )
        .expect("write shared graph host");
        let mut compile_options = CompileOptions::default();
        compile_options.include_dirs.push(root.path.clone());
        toolchain
            .compile_c(&host_source, &host_object, &compile_options)
            .expect("compile shared graph host");
        let mut link_options = LinkOptions::default();
        link_options.runtime_libraries.push(report.output.clone());
        let loader_directory = report.loader.parent().expect("loader has directory");
        link_options
            .extra_args
            .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
        toolchain
            .link_executable(&[host_object], &host_executable, &link_options)
            .expect("link shared graph host");
        let mut command = Command::new(&host_executable);
        #[cfg(target_os = "linux")]
        command.env("LD_LIBRARY_PATH", loader_directory);
        #[cfg(target_os = "macos")]
        command.env("DYLD_LIBRARY_PATH", loader_directory);
        let output = command.output().expect("run shared graph host");
        assert!(
            output.status.success(),
            "shared graph host failed with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn content_named_loaders_allow_same_basename_libraries_in_one_host() {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            eprintln!("skipping shared loader integration on unsupported platform");
            return;
        }
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping shared loader integration: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-content-loader-test");
        let first_dir = root.path.join("first");
        let second_dir = root.path.join("second");
        let leaf = if cfg!(target_os = "macos") {
            "same.dylib"
        } else {
            "same.so"
        };
        let program = |value: f64| Program {
            version: BYTECODE_VERSION,
            constants: vec![Constant::Number(value.to_bits())],
            code: vec![Instruction::Const(0), Instruction::Return],
            local_count: 0,
            functions: Vec::new(),
        };
        let builder = NativeBuilder::new(
            ArtifactCache::new(root.path.join("cache")),
            toolchain.clone(),
        );
        let first = builder
            .build_shared_library(
                &program(11.0),
                &NativeBuildOptions {
                    output: first_dir.join(leaf),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("build first same-basename library");
        let second = builder
            .build_shared_library(
                &program(22.0),
                &NativeBuildOptions {
                    output: second_dir.join(leaf),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("build second same-basename library");
        assert_ne!(first.loader.file_name(), second.loader.file_name());
        assert_eq!(
            fs::read(&first.output).unwrap(),
            fs::read(&first.loader).unwrap()
        );
        assert_eq!(
            fs::read(&second.output).unwrap(),
            fs::read(&second.loader).unwrap()
        );

        let host_source = root.path.join("host.c");
        fs::write(
            &host_source,
            format!(
                r#"#include "first/same.h"
#include "second/same.h"
#include <stdint.h>

int main(void) {{
    kiln_value result;
    if ({first_symbol}(NULL, &result) != KILN_C99_OK ||
        result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({first_bits}))
        return 1;
    if ({second_symbol}(NULL, &result) != KILN_C99_OK ||
        result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({second_bits}))
        return 2;
    return 0;
}}
"#,
                first_symbol = first.module_symbol,
                second_symbol = second.module_symbol,
                first_bits = 11.0_f64.to_bits(),
                second_bits = 22.0_f64.to_bits(),
            ),
        )
        .expect("write same-basename host");
        let host_object = root.path.join("host.o");
        let mut compile_options = CompileOptions::default();
        compile_options.include_dirs.push(root.path.clone());
        toolchain
            .compile_c(&host_source, &host_object, &compile_options)
            .expect("compile same-basename host");
        let host_executable = root.path.join("host");
        let mut link_options = LinkOptions {
            runtime_libraries: vec![first.output.clone(), second.output.clone()],
            ..LinkOptions::default()
        };
        for directory in [
            first.loader.parent().expect("first loader directory"),
            second.loader.parent().expect("second loader directory"),
        ] {
            link_options
                .extra_args
                .push(format!("-Wl,-rpath,{}", directory.display()).into());
        }
        toolchain
            .link_executable(&[host_object], &host_executable, &link_options)
            .expect("link same-basename host");
        let output = Command::new(&host_executable)
            .output()
            .expect("run same-basename host");
        assert!(
            output.status.success(),
            "same-basename host failed with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn shared_graph_libraries_hide_path_symbols_and_cannot_interpose_each_other() {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            eprintln!("skipping shared graph visibility integration on unsupported platform");
            return;
        }
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping shared graph visibility integration: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-shared-graph-visibility-test");
        let first_graph = one_module_graph(&root.path.join("first-project"), 11.0, "first");
        let second_graph = one_module_graph(&root.path.join("second-project"), 22.0, "second");
        let first_internal = emit_module_with_logical_path(
            &first_graph
                .modules()
                .next()
                .expect("first graph module")
                .program,
            "main.js",
        )
        .expect("first module emits")
        .symbol;
        let second_internal = emit_module_with_logical_path(
            &second_graph
                .modules()
                .next()
                .expect("second graph module")
                .program,
            "main.js",
        )
        .expect("second module emits")
        .symbol;
        assert_eq!(
            first_internal, second_internal,
            "logical-path entries must collide for this interposition regression"
        );

        let builder = NativeBuilder::new(
            ArtifactCache::new(root.path.join("cache")),
            toolchain.clone(),
        );
        let first = builder
            .build_shared_graph(
                &first_graph,
                &NativeBuildOptions {
                    output: root.path.join(if cfg!(target_os = "macos") {
                        "first.dylib"
                    } else {
                        "first.so"
                    }),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("build first graph library");
        let second = builder
            .build_shared_graph(
                &second_graph,
                &NativeBuildOptions {
                    output: root.path.join(if cfg!(target_os = "macos") {
                        "second.dylib"
                    } else {
                        "second.so"
                    }),
                    profile: NativeProfile::Dev,
                },
            )
            .expect("build second graph library");
        assert_ne!(
            first.module_symbol, second.module_symbol,
            "the host must be able to address both public wrappers independently"
        );

        let first_header = first
            .header
            .file_name()
            .and_then(|name| name.to_str())
            .expect("first header name");
        let second_header = second
            .header
            .file_name()
            .and_then(|name| name.to_str())
            .expect("second header name");
        let host_source = root.path.join("host.c");
        fs::write(
            &host_source,
            format!(
                r#"#include "{first_header}"
#include "{second_header}"
#include <dlfcn.h>
#include <stdint.h>
#include <string.h>

typedef int (*kiln_entry_fn)(kiln_host *, kiln_value *);

static kiln_entry_fn load_entry(void *handle, const char *name) {{
    void *address = dlsym(handle, name);
    kiln_entry_fn entry = NULL;
    if (address == NULL) return NULL;
    _Static_assert(sizeof(entry) == sizeof(address), "dlsym function pointer size");
    memcpy(&entry, &address, sizeof(entry));
    return entry;
}}

int main(int argc, char **argv) {{
    void *first;
    void *second;
    kiln_entry_fn first_entry;
    kiln_entry_fn second_entry;
    kiln_value result;
    if (argc != 3) return 1;
    first = dlopen(argv[1], RTLD_NOW | RTLD_GLOBAL);
    if (first == NULL) return 2;
    second = dlopen(argv[2], RTLD_NOW | RTLD_LOCAL);
    if (second == NULL) return 3;
    if (dlsym(first, "{internal}") != NULL || dlsym(second, "{internal}") != NULL ||
        dlsym(first, "kiln_number_to_js_string") != NULL ||
        dlsym(second, "kiln_number_to_js_string") != NULL ||
        dlsym(first, "kiln_string_to_number") != NULL ||
        dlsym(second, "kiln_string_to_number") != NULL ||
        dlsym(first, "rust_eh_personality") != NULL ||
        dlsym(second, "rust_eh_personality") != NULL ||
        dlsym(first, "rust_begin_unwind") != NULL ||
        dlsym(second, "rust_begin_unwind") != NULL ||
        dlsym(first, "ffc_parse_double") != NULL ||
        dlsym(second, "ffc_parse_double") != NULL)
        return 4;
    first_entry = load_entry(first, "{first_symbol}");
    second_entry = load_entry(second, "{second_symbol}");
    if (first_entry == NULL || second_entry == NULL) return 5;
    if (first_entry(NULL, &result) != KILN_C99_OK ||
        result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({first_bits}))
        return 6;
    if (second_entry(NULL, &result) != KILN_C99_OK ||
        result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({second_bits}))
        return 7;
    dlclose(second);
    dlclose(first);
    return 0;
}}
"#,
                internal = first_internal,
                first_symbol = first.module_symbol,
                second_symbol = second.module_symbol,
                first_bits = 11.0_f64.to_bits(),
                second_bits = 22.0_f64.to_bits(),
            ),
        )
        .expect("write visibility host");
        let host_object = root.path.join("host.o");
        let mut compile_options = CompileOptions::default();
        compile_options.include_dirs.push(root.path.clone());
        toolchain
            .compile_c(&host_source, &host_object, &compile_options)
            .expect("compile visibility host");
        let host_executable = root.path.join("host");
        let link_options = LinkOptions {
            libraries: if cfg!(target_os = "linux") {
                vec!["dl".to_owned()]
            } else {
                Vec::new()
            },
            ..LinkOptions::default()
        };
        toolchain
            .link_executable(&[host_object], &host_executable, &link_options)
            .expect("link visibility host");
        let output = Command::new(&host_executable)
            .arg(&first.output)
            .arg(&second.output)
            .output()
            .expect("run visibility host");
        assert!(
            output.status.success(),
            "shared graph visibility host failed with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn stdout_program(text: &str, result: f64) -> Program {
        Program {
            version: BYTECODE_VERSION,
            constants: vec![
                Constant::String(text.to_owned()),
                Constant::Number(result.to_bits()),
            ],
            code: vec![
                Instruction::Const(0),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Pop,
                Instruction::Const(1),
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        }
    }

    fn scalar_equivalence_programs() -> Vec<(&'static str, Program)> {
        fn push_constant(constants: &mut Vec<Constant>, value: Constant) -> u32 {
            let index = constants.len() as u32;
            constants.push(value);
            index
        }

        fn push_unary_result(
            constants: &mut Vec<Constant>,
            code: &mut Vec<Instruction>,
            operator: kiln_ir::UnaryOp,
            input: Constant,
        ) {
            let input = push_constant(constants, input);
            code.extend([Instruction::Const(input), Instruction::Unary(operator)]);
        }

        fn push_binary_result(
            constants: &mut Vec<Constant>,
            code: &mut Vec<Instruction>,
            operator: kiln_ir::BinaryOp,
            left: Constant,
            right: Constant,
        ) {
            let left = push_constant(constants, left);
            let right = push_constant(constants, right);
            code.extend([
                Instruction::Const(left),
                Instruction::Const(right),
                Instruction::Binary(operator),
            ]);
        }

        let string_numeric_inputs = [
            "",
            "\u{00a0}\u{1680}\u{2000}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}1\u{feff}",
            "\u{0085}1\u{0085}",
            "-0",
            "Infinity",
            "+Infinity",
            "-Infinity",
            "inf",
            "1e309",
            "-1e-9999",
            "5e-324",
            "0x10",
            "0b11",
            "0o10",
            "0x20000000000001",
            "0x20000000000003",
            "+0x10",
            "0x1p2",
            "1e+",
            "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        ];
        let mut string_numeric_code = Vec::with_capacity(string_numeric_inputs.len() * 2 + 2);
        for index in 0..string_numeric_inputs.len() {
            string_numeric_code.push(Instruction::Const(index as u32));
            string_numeric_code.push(Instruction::Unary(kiln_ir::UnaryOp::Plus));
        }
        string_numeric_code.push(Instruction::HostCall {
            op: HostOp::WriteStdout,
            argc: string_numeric_inputs.len() as u16,
        });
        string_numeric_code.push(Instruction::Return);
        let string_numeric_program = Program {
            version: BYTECODE_VERSION,
            constants: string_numeric_inputs
                .into_iter()
                .map(|value| Constant::String(value.to_owned()))
                .collect(),
            code: string_numeric_code,
            local_count: 0,
            functions: Vec::new(),
        };

        let number = |value: f64| Constant::Number(value.to_bits());
        let mut operator_constants = Vec::new();
        let mut operator_code = Vec::new();
        let mut operator_results = 0;
        for input in [
            Constant::Undefined,
            Constant::Null,
            Constant::Boolean(true),
            number(1.0),
            Constant::String("x".to_owned()),
        ] {
            push_unary_result(
                &mut operator_constants,
                &mut operator_code,
                kiln_ir::UnaryOp::TypeOf,
                input,
            );
            operator_results += 1;
        }
        for (operator, input) in [
            (kiln_ir::UnaryOp::BitwiseNot, number(5.0)),
            (kiln_ir::UnaryOp::BitwiseNot, number(4_294_967_296.0)),
            (kiln_ir::UnaryOp::Void, number(42.0)),
        ] {
            push_unary_result(&mut operator_constants, &mut operator_code, operator, input);
            operator_results += 1;
        }
        for (operator, left, right) in [
            (
                kiln_ir::BinaryOp::LooseEqual,
                Constant::Null,
                Constant::Undefined,
            ),
            (
                kiln_ir::BinaryOp::LooseEqual,
                number(0.0),
                Constant::Boolean(false),
            ),
            (
                kiln_ir::BinaryOp::LooseEqual,
                Constant::String("42".to_owned()),
                number(42.0),
            ),
            (
                kiln_ir::BinaryOp::LooseEqual,
                Constant::String(String::new()),
                number(0.0),
            ),
            (kiln_ir::BinaryOp::LooseEqual, Constant::Null, number(0.0)),
            (
                kiln_ir::BinaryOp::LooseNotEqual,
                number(f64::NAN),
                number(f64::NAN),
            ),
            (kiln_ir::BinaryOp::BitwiseAnd, number(5.0), number(3.0)),
            (kiln_ir::BinaryOp::BitwiseOr, number(5.0), number(2.0)),
            (kiln_ir::BinaryOp::BitwiseXor, number(5.0), number(3.0)),
            (kiln_ir::BinaryOp::ShiftLeft, number(1.0), number(31.0)),
            (kiln_ir::BinaryOp::ShiftLeft, number(1.0), number(32.0)),
            (kiln_ir::BinaryOp::ShiftRight, number(-1.0), number(1.0)),
            (
                kiln_ir::BinaryOp::UnsignedShiftRight,
                number(-1.0),
                number(1.0),
            ),
            (
                kiln_ir::BinaryOp::BitwiseOr,
                number(4_294_967_296.0),
                number(0.0),
            ),
            (kiln_ir::BinaryOp::BitwiseOr, number(-1.9), number(0.0)),
            (
                kiln_ir::BinaryOp::BitwiseOr,
                number(f64::INFINITY),
                number(0.0),
            ),
        ] {
            push_binary_result(
                &mut operator_constants,
                &mut operator_code,
                operator,
                left,
                right,
            );
            operator_results += 1;
        }
        operator_code.extend([
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: operator_results,
            },
            Instruction::Return,
        ]);
        let operator_program = Program {
            version: BYTECODE_VERSION,
            constants: operator_constants,
            code: operator_code,
            local_count: 0,
            functions: Vec::new(),
        };

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
                    functions: Vec::new(),
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
                    functions: Vec::new(),
                },
            ),
            (
                "number-formatting",
                Program {
                    version: BYTECODE_VERSION,
                    constants: vec![
                        Constant::String("v=".to_owned()),
                        Constant::Number(0.1_f64.to_bits()),
                        Constant::Number((1.0_f64 / 3.0).to_bits()),
                        Constant::Number((-0.0_f64).to_bits()),
                        Constant::Number(1e20_f64.to_bits()),
                        Constant::Number(1e21_f64.to_bits()),
                        Constant::Number(1e-6_f64.to_bits()),
                        Constant::Number(1e-7_f64.to_bits()),
                        Constant::Number(1.2345e-7_f64.to_bits()),
                        Constant::Number(f64::from_bits(1).to_bits()),
                        Constant::Number(f64::NAN.to_bits()),
                        Constant::Number(f64::INFINITY.to_bits()),
                        Constant::Number(f64::NEG_INFINITY.to_bits()),
                    ],
                    code: vec![
                        // Keep the direct-number call separate from the concatenated call: the
                        // former exercises console formatting (including negative zero), while
                        // the latter exercises Number-to-string conversion in the C backend.
                        Instruction::Const(1),
                        Instruction::Const(2),
                        Instruction::Const(3),
                        Instruction::Const(4),
                        Instruction::Const(5),
                        Instruction::Const(6),
                        Instruction::Const(7),
                        Instruction::Const(8),
                        Instruction::Const(9),
                        Instruction::Const(10),
                        Instruction::Const(11),
                        Instruction::Const(12),
                        Instruction::HostCall {
                            op: HostOp::WriteStdout,
                            argc: 12,
                        },
                        Instruction::Pop,
                        Instruction::Const(0),
                        Instruction::Const(1),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::Const(0),
                        Instruction::Const(2),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::Const(0),
                        Instruction::Const(3),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::Const(0),
                        Instruction::Const(4),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::Const(0),
                        Instruction::Const(5),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::Const(0),
                        Instruction::Const(6),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::Const(0),
                        Instruction::Const(7),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::Const(0),
                        Instruction::Const(8),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::Const(0),
                        Instruction::Const(9),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::Const(0),
                        Instruction::Const(10),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::Const(0),
                        Instruction::Const(11),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::Const(0),
                        Instruction::Const(12),
                        Instruction::Binary(kiln_ir::BinaryOp::Add),
                        Instruction::HostCall {
                            op: HostOp::WriteStdout,
                            argc: 12,
                        },
                        Instruction::Return,
                    ],
                    local_count: 0,
                    functions: Vec::new(),
                },
            ),
            ("scalar-operators", operator_program),
            ("string-numeric-literals", string_numeric_program),
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
                    functions: Vec::new(),
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
                    functions: Vec::new(),
                },
            ),
        ]
    }
}
