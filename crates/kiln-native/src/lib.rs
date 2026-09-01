//! Cached orchestration for Kiln's C native backend.
//!
//! The backend, runtime, cache, and platform toolchain deliberately live in separate crates.
//! This crate is the narrow coordinator that gives those pieces an ordered build graph:
//! generated module C, application object, reusable runtime object, launcher object, executable.

mod number_provider;
pub mod shared_publication;

use std::collections::BTreeSet;
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
#[cfg(test)]
use kiln_backend_c::emit_module_with_logical_path;
use kiln_backend_c::{
    C_ABI_VERSION, C_GENERATOR_VERSION, EmissionPolicy, EmissionStringPolicy, EmitError,
    emit_commonjs_module_with_logical_path_and_policy, emit_module_with_logical_path_and_policy,
    emit_runtime_module_with_logical_path, emit_with_policy,
};
use kiln_host_abi::HOST_ABI_VERSION;
pub use kiln_host_abi::HostValue;
use kiln_ir::{ExportBinding, ModuleProgram, Program};
use kiln_module_graph::{ModuleGraph, ModuleGraphError, ModuleSource, ResolvedExport};
use kiln_native_runtime::{
    EMBEDDING_C_ABI_VERSION, EMBEDDING_HOST_ABI_VERSION, EMBEDDING_MODULE_API_VERSION,
    NATIVE_RESULT_PATH_ENV, NATIVE_RUNTIME_VERSION, RuntimeSourceError, decode_native_result,
    embedding_module_api_symbol, launcher_source, runtime_source,
};
pub use kiln_native_runtime::{embedding_header, embedding_module_header};
use kiln_native_toolchain::{
    BuildProfile as ToolchainProfile, CompileOptions, CompilerKind, LinkOptions, ToolIdentity,
    Toolchain, ToolchainError,
};
use number_provider::{NumberProvider, PROVIDER_ABI_VERSION, PROVIDER_SCHEMA_VERSION};
use rayon::prelude::*;

static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);
static MATERIALIZE_COUNTER: AtomicU64 = AtomicU64::new(0);
static DISCOVERED_TOOLCHAIN: OnceLock<Result<Toolchain, ToolchainError>> = OnceLock::new();
static SHARED_PUBLICATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

const PRIVATE_ASYNC_ABI_VERSION: u32 = 5;

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
    pub realm: NativeRealmPolicy,
}

impl NativeBuildOptions {
    pub fn new(output: impl Into<PathBuf>, profile: NativeProfile) -> Self {
        Self {
            output: output.into(),
            profile,
            realm: NativeRealmPolicy::default(),
        }
    }

    pub fn with_realm_policy(mut self, realm: NativeRealmPolicy) -> Self {
        self.realm = realm;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeRealmPolicy {
    pub host_clock: bool,
    pub entropy: NativeEntropyPolicy,
    pub timezone: NativeStringPolicy,
    pub locale: NativeStringPolicy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NativeEntropyPolicy {
    #[default]
    Deterministic,
    Seeded(u64),
    Host,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NativeStringPolicy {
    #[default]
    Deterministic,
    Named(String),
    Host,
}

impl NativeRealmPolicy {
    pub const fn host_clock() -> Self {
        Self {
            host_clock: true,
            entropy: NativeEntropyPolicy::Deterministic,
            timezone: NativeStringPolicy::Deterministic,
            locale: NativeStringPolicy::Deterministic,
        }
    }

    pub const fn with_entropy(mut self, entropy: NativeEntropyPolicy) -> Self {
        self.entropy = entropy;
        self
    }

    pub fn with_timezone(mut self, timezone: NativeStringPolicy) -> Self {
        self.timezone = timezone;
        self
    }

    pub fn with_locale(mut self, locale: NativeStringPolicy) -> Self {
        self.locale = locale;
        self
    }
}

impl NativeRealmPolicy {
    fn emission(&self) -> EmissionPolicy {
        EmissionPolicy {
            host_clock: self.host_clock,
            entropy: match self.entropy {
                NativeEntropyPolicy::Deterministic => {
                    kiln_backend_c::EmissionEntropyPolicy::Deterministic
                }
                NativeEntropyPolicy::Seeded(seed) => {
                    kiln_backend_c::EmissionEntropyPolicy::Seeded(seed)
                }
                NativeEntropyPolicy::Host => kiln_backend_c::EmissionEntropyPolicy::Host,
            },
            timezone: match &self.timezone {
                NativeStringPolicy::Deterministic => EmissionStringPolicy::Deterministic,
                NativeStringPolicy::Named(value) => EmissionStringPolicy::Named(value.clone()),
                NativeStringPolicy::Host => EmissionStringPolicy::Host,
            },
            locale: match &self.locale {
                NativeStringPolicy::Deterministic => EmissionStringPolicy::Deterministic,
                NativeStringPolicy::Named(value) => EmissionStringPolicy::Named(value.clone()),
                NativeStringPolicy::Host => EmissionStringPolicy::Host,
            },
        }
    }
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
    /// Program-specific C header declaring both the legacy entry and owned-result API accessor.
    pub header: PathBuf,
    /// Legacy ABI-v1 entry point with a borrowed scalar result.
    pub module_symbol: String,
    /// ABI-v2 descriptor accessor with an explicitly owned scalar result.
    pub module_api_symbol: String,
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
    UnsupportedNodeBuiltin {
        specifier: &'static str,
    },
    UnsupportedNodeBuiltinExport {
        specifier: &'static str,
        export_name: String,
    },
    SharedPackage(shared_publication::SharedPackageError),
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
            Self::UnsupportedNodeBuiltin { specifier } => write!(
                formatter,
                "native Node execution does not yet materialize the `{specifier}` builtin"
            ),
            Self::UnsupportedNodeBuiltinExport {
                specifier,
                export_name,
            } => write!(
                formatter,
                "native Node execution does not yet materialize the `{specifier}` export `{export_name}`"
            ),
            Self::SharedPackage(error) => {
                write!(formatter, "native shared-library package failed: {error}")
            }
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
            | Self::RunResult { .. }
            | Self::UnsupportedNodeBuiltin { .. }
            | Self::UnsupportedNodeBuiltinExport { .. } => None,
            Self::SharedPackage(error) => Some(error),
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

impl From<shared_publication::SharedPackageError> for NativeError {
    fn from(value: shared_publication::SharedPackageError) -> Self {
        Self::SharedPackage(value)
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

    /// Every artifact identity this configured native toolchain can still produce.
    ///
    /// Cache eviction deliberately asks the producer for this set instead of reconstructing the
    /// native build matrix in the daemon. The identity digest excludes per-program inputs and
    /// options, so one representative key covers every artifact in a stage/profile family.
    /// Keeping the catalogue here also means ABI, backend, or profile additions must change the
    /// same Module that constructs the real keys.
    pub fn live_cache_identities(&self) -> Result<BTreeSet<Digest>, NativeError> {
        let tool = self.toolchain.identity();
        let target = target_identity(tool);
        let compiler = compiler_identity(tool);
        let module_abi = module_abi_identity();
        let runtime_abi = runtime_abi_identity();
        let embedding_abi = shared_embedding_abi_identity();
        let provider = self.number_provider()?;
        let mut identities = BTreeSet::new();

        let mut allow = |compiler: String, backend: Backend, abi: &str, profile| {
            identities.insert(cache_identity(compiler, backend, abi, &target, profile));
        };

        // Deterministic generated sources are always stored under the debug cache profile. Graph
        // module sources share the ordinary module-source identity byte-for-byte.
        allow(
            format!("kiln-c-abi-{C_ABI_VERSION}"),
            Backend::C,
            &module_abi,
            CacheProfile::Debug,
        );
        allow(
            format!("kiln-runtime-{NATIVE_RUNTIME_VERSION}"),
            Backend::Custom("native-runtime-source".to_owned()),
            &runtime_abi,
            CacheProfile::Debug,
        );
        allow(
            format!("kiln-launcher-{NATIVE_RUNTIME_VERSION}"),
            Backend::Custom("native-launcher-source".to_owned()),
            &runtime_abi,
            CacheProfile::Debug,
        );
        allow(
            format!("kiln-graph-launcher-{NATIVE_RUNTIME_VERSION}"),
            Backend::Custom("native-graph-launcher-source".to_owned()),
            &runtime_abi,
            CacheProfile::Debug,
        );
        allow(
            format!("kiln-c-abi-{C_ABI_VERSION}-module-api-{EMBEDDING_MODULE_API_VERSION}"),
            Backend::Custom("native-shared-library-source".to_owned()),
            &embedding_abi,
            CacheProfile::Debug,
        );
        allow(
            format!(
                "kiln-shared-graph-wrapper-{NATIVE_RUNTIME_VERSION}-module-api-{EMBEDDING_MODULE_API_VERSION}"
            ),
            Backend::Custom("native-shared-graph-wrapper-source".to_owned()),
            &runtime_abi,
            CacheProfile::Debug,
        );

        // The Rust number provider is profile-independent and is cached as one release archive.
        allow(
            provider.identity,
            Backend::Custom("rust-native-number-provider".to_owned()),
            &native_number_abi_identity(),
            CacheProfile::Release,
        );

        for profile in [
            NativeProfile::Dev,
            NativeProfile::Check,
            NativeProfile::Release,
        ] {
            let profile = profile.cache();
            for (backend, abi) in [
                (Backend::C, module_abi.as_str()),
                (
                    Backend::Custom("native-runtime".to_owned()),
                    runtime_abi.as_str(),
                ),
                (
                    Backend::Custom("native-launcher".to_owned()),
                    runtime_abi.as_str(),
                ),
                (Backend::C, runtime_abi.as_str()),
                (
                    Backend::Custom("native-shared-library".to_owned()),
                    embedding_abi.as_str(),
                ),
                (
                    Backend::Custom("native-shared-graph-module".to_owned()),
                    module_abi.as_str(),
                ),
                (
                    Backend::Custom("native-shared-graph-wrapper".to_owned()),
                    runtime_abi.as_str(),
                ),
                (
                    Backend::Custom("native-shared-graph".to_owned()),
                    runtime_abi.as_str(),
                ),
                (
                    Backend::Custom("native-graph-launcher".to_owned()),
                    runtime_abi.as_str(),
                ),
            ] {
                allow(compiler.clone(), backend, abi, profile.clone());
            }
        }

        Ok(identities)
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
        let module = emit_with_policy(program, options.realm.emission())?;
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

        let object_options = Digest::hash(
            format!(
                "c11\0{}\0{}\0generator-{C_GENERATOR_VERSION}",
                identity.kind,
                profile.as_str()
            )
            .as_bytes(),
        );
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
        let ((application_result, runtime_result), launcher_result) = rayon::join(
            || {
                rayon::join(
                    || {
                        run_worker("application compile", || {
                            self.object(
                                &application_key,
                                &module_source,
                                &paths.module_c,
                                &paths.module_o,
                                &compile_options,
                            )
                        })
                    },
                    || {
                        run_worker("runtime compile", || {
                            self.object(
                                &runtime_key,
                                &runtime_bytes,
                                &paths.runtime_c,
                                &paths.runtime_o,
                                &compile_options,
                            )
                        })
                    },
                )
            },
            || {
                run_worker("launcher compile", || {
                    self.object(
                        &launcher_key,
                        &launcher_bytes,
                        &paths.launcher_c,
                        &paths.launcher_o,
                        &compile_options,
                    )
                })
            },
        );
        let (application, application_digest, application_hit, _application_elapsed) =
            application_result?;
        let (runtime_object, runtime_digest, runtime_hit, _runtime_elapsed) = runtime_result?;
        let (launcher_object, launcher_digest, launcher_hit, _launcher_elapsed) = launcher_result?;
        let compile_elapsed = if application_hit && runtime_hit && launcher_hit {
            Duration::ZERO
        } else {
            compile_started.elapsed()
        };

        let link_options_digest = Digest::hash(
            format!("{}\0system-math\0system-entropy-v1", profile.as_str()).as_bytes(),
        );
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
            match self.lookup(&executable_key)? {
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
                    } else {
                        link_options.libraries.push("bcrypt".to_owned());
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
    /// The resulting library exports the module's legacy entry point plus its owned-result API-v2
    /// descriptor accessor and expects the caller to provide the stable `kiln_host` callback ABI.
    /// It deliberately does not link the CLI runtime or launcher, so host policy remains in the
    /// embedding process.
    pub fn build_shared_library(
        &self,
        program: &Program,
        options: &NativeBuildOptions,
    ) -> Result<NativeSharedLibraryReport, NativeError> {
        let total_started = Instant::now();
        let emit_started = Instant::now();
        let module = emit_with_policy(program, options.realm.emission())?;
        let module_api_symbol = embedding_module_api_symbol(&module.symbol)?;
        let module_wrapper = owned_result_wrapper_source(
            &module.symbol,
            "scalar",
            &format!("{}(&proxy_host, &value)", module.symbol),
            "",
        );
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
        let embedding_abi = shared_embedding_abi_identity();
        let target = target_identity(&identity);
        let compiler = compiler_identity(&identity);
        let (number_provider, number_provider_digest, _) = self.cached_number_provider(&target)?;
        // Keep the plain generated unit on the same cache identity as executable builds.  The
        // shared-only adapter is cached as a second generated-C artifact, so building a DSO does
        // not poison the reusable non-PIC module source cache.
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
        let (base_source, base_source_digest, _) =
            self.cached_payload(&module_source_key, module.source.as_bytes())?;
        let shared_source_key = key(
            ArtifactKind::GeneratedC,
            format!("kiln-c-abi-{C_ABI_VERSION}-module-api-{EMBEDDING_MODULE_API_VERSION}"),
            Backend::Custom("native-shared-library-source".to_owned()),
            &embedding_abi,
            &target,
            CacheProfile::Debug,
            Digest::hash(b"deterministic-c11-owned-result-v2"),
            [base_source_digest, Digest::hash(module_wrapper.as_bytes())],
        );
        let mut shared_module_source = base_source;
        shared_module_source.push(b'\n');
        shared_module_source.extend_from_slice(module_wrapper.as_bytes());
        let (module_source, module_source_digest, generated_c_hit) =
            self.cached_payload(&shared_source_key, &shared_module_source)?;
        let object_options = Digest::hash(
            format!(
                "shared-library\0pic\0c11\0{}\0{}\0generator-{C_GENERATOR_VERSION}",
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
                &[module.symbol.as_str(), module_api_symbol.as_str()],
                link_inputs,
            ),
        )?;
        let link_options_digest = Digest::hash(
            format!(
                "shared-library\0{}\0system-math\0{}\0{}\0{}\0module-api-v{}\0exports={}",
                profile.as_str(),
                shared_library_name(),
                shared_library_link_identity(&loader),
                shared_library_link_policy(),
                EMBEDDING_MODULE_API_VERSION,
                [module.symbol.as_str(), module_api_symbol.as_str()].join(","),
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
        let (library, library_hit, link_elapsed) = match self.lookup(&library_key)? {
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
                    &[module.symbol.as_str(), module_api_symbol.as_str()],
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
        publish_shared_artifacts_with_symbols(
            &options.output,
            &loader,
            header_source.as_bytes(),
            &library,
            &module.symbol,
            &module_api_symbol,
        )?;

        Ok(NativeSharedLibraryReport {
            output: options.output.clone(),
            loader,
            header,
            module_symbol: module.symbol,
            module_api_symbol,
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
        validate_native_node_builtins(graph)?;
        let total_started = Instant::now();
        let emit_started = Instant::now();
        let mut modules = Vec::with_capacity(graph.module_count());
        let mut symbols = std::collections::HashSet::with_capacity(graph.module_count());
        for node in graph.modules() {
            let logical_path = logical_module_path(graph.project_root(), &node.path)?;
            let emitted = emit_native_graph_module(node, &logical_path, &options.realm)?;
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
        let wrapper_symbol = format!(
            "kiln_shared_graph_{}",
            graph_shared_public_digest(topology, &modules)?.to_hex()
        );
        let wrapper = graph_shared_wrapper_source(graph, &modules, &wrapper_symbol)?;
        let module_api_symbol = embedding_module_api_symbol(&wrapper_symbol)?;
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
            format!(
                "kiln-shared-graph-wrapper-{NATIVE_RUNTIME_VERSION}-module-api-{EMBEDDING_MODULE_API_VERSION}"
            ),
            Backend::Custom("native-shared-graph-wrapper-source".to_owned()),
            &embedding_abi,
            &target,
            CacheProfile::Debug,
            Digest::hash(b"deterministic-c11-shared-graph-wrapper-owned-result-v2"),
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
                &module_abi,
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
        let (module_objects, wrapper_object) = rayon::join(
            || {
                let module_keys_ref = &module_keys;
                let module_sources_ref = &module_sources;
                let module_c_ref = &paths.module_c;
                let module_o_ref = &paths.module_o;
                let module_compile_options_ref = &module_compile_options;
                compile_indexed(module_keys_ref.len(), "module compile", move |index| {
                    self.object(
                        &module_keys_ref[index],
                        &module_sources_ref[index].0,
                        &module_c_ref[index],
                        &module_o_ref[index],
                        module_compile_options_ref,
                    )
                    .map(|(object, digest, hit, _elapsed)| (object, digest, hit))
                })
            },
            || {
                run_worker("shared graph wrapper compile", || {
                    self.object(
                        &wrapper_object_key,
                        &wrapper_bytes,
                        &paths.wrapper_c,
                        &paths.wrapper_o,
                        &wrapper_compile_options,
                    )
                })
            },
        );
        let module_objects = module_objects?;
        let wrapper_object = wrapper_object?;
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
                &[wrapper_symbol.as_str(), module_api_symbol.as_str()],
                link_inputs.iter().copied(),
            ),
        )?;
        let link_options_digest = Digest::hash(
            format!(
                "shared-graph\0{}\0system-math\0{}\0{}\0{}\0module-api-v{}\0exports={}",
                profile.as_str(),
                shared_library_name(),
                shared_library_link_identity(&loader),
                shared_library_link_policy(),
                EMBEDDING_MODULE_API_VERSION,
                [wrapper_symbol.as_str(), module_api_symbol.as_str()].join(","),
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
        let (library, library_hit, link_elapsed) = match self.lookup(&library_key)? {
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
                    &[wrapper_symbol.as_str(), module_api_symbol.as_str()],
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
        publish_shared_artifacts_with_symbols(
            &options.output,
            &loader,
            header_source.as_bytes(),
            &library,
            &wrapper_symbol,
            &module_api_symbol,
        )?;

        Ok(NativeSharedLibraryReport {
            output: options.output.clone(),
            loader,
            header,
            module_symbol: wrapper_symbol,
            module_api_symbol,
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
        validate_native_node_builtins(graph)?;
        let total_started = Instant::now();
        let emit_started = Instant::now();
        let mut modules = Vec::with_capacity(graph.module_count());
        let mut logical_paths = Vec::with_capacity(graph.module_count());
        let mut symbols = std::collections::HashSet::with_capacity(graph.module_count());
        for node in graph.modules() {
            let logical_path = logical_module_path(graph.project_root(), &node.path)?;
            let emitted = emit_native_graph_module(node, &logical_path, &options.realm)?;
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

        let object_options = Digest::hash(
            format!(
                "c11\0{}\0{}\0generator-{C_GENERATOR_VERSION}",
                identity.kind,
                profile.as_str()
            )
            .as_bytes(),
        );
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
        let (module_objects, (runtime_object, launcher_object)) = rayon::join(
            || {
                let module_keys_ref = &module_keys;
                let module_sources_ref = &module_sources;
                let module_c_ref = &paths.module_c;
                let module_o_ref = &paths.module_o;
                let compile_options_ref = &compile_options;
                compile_indexed(module_keys_ref.len(), "module compile", move |index| {
                    self.object(
                        &module_keys_ref[index],
                        &module_sources_ref[index].0,
                        &module_c_ref[index],
                        &module_o_ref[index],
                        compile_options_ref,
                    )
                    .map(|(object, digest, hit, _elapsed)| (object, digest, hit))
                })
            },
            || {
                rayon::join(
                    || {
                        run_worker("runtime compile", || {
                            self.object(
                                &runtime_key,
                                &runtime_bytes,
                                &paths.runtime_c,
                                &paths.runtime_o,
                                &compile_options,
                            )
                        })
                    },
                    || {
                        run_worker("launcher compile", || {
                            self.object(
                                &launcher_key,
                                &launcher_bytes,
                                &paths.launcher_c,
                                &paths.launcher_o,
                                &compile_options,
                            )
                        })
                    },
                )
            },
        );
        let module_objects = module_objects?;
        let runtime_object = runtime_object?;
        let launcher_object = launcher_object?;
        let (runtime_object, runtime_digest, runtime_hit, _) = runtime_object;
        let (launcher_object, launcher_digest, launcher_hit, _) = launcher_object;
        let compile_elapsed =
            if module_objects.iter().all(|(_, _, hit)| *hit) && runtime_hit && launcher_hit {
                Duration::ZERO
            } else {
                compile_started.elapsed()
            };

        let link_options_digest = Digest::hash(
            format!("{}\0system-math\0system-entropy-v1", profile.as_str()).as_bytes(),
        );
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
                } else {
                    link_options.libraries.push("bcrypt".to_owned());
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
        #[cfg(test)]
        self.assert_live_cache_identity(key);
        cached_payload(&self.cache, key, expected)
    }

    fn lookup(&self, key: &ArtifactKey) -> Result<Lookup, NativeError> {
        #[cfg(test)]
        self.assert_live_cache_identity(key);
        Ok(self.cache.lookup(key)?)
    }

    #[cfg(test)]
    fn assert_live_cache_identity(&self, key: &ArtifactKey) {
        let identities = self
            .live_cache_identities()
            .expect("a build that constructs cache keys can enumerate its live identities");
        assert!(
            identities.contains(&key.identity_digest()),
            "native cache identity catalogue omitted key family: {key:?}"
        );
    }

    fn object(
        &self,
        key: &ArtifactKey,
        source: &[u8],
        source_path: &Path,
        object_path: &Path,
        options: &CompileOptions,
    ) -> Result<(Vec<u8>, Digest, bool, Duration), NativeError> {
        match self.lookup(key)? {
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

/// Fail before emission if a typed builtin has no native materializer.
///
/// A builtin node's bytecode is intentionally empty: its exports are supplied by the execution
/// adapter. Treating it as an ordinary generated module therefore succeeds while leaving every
/// export `undefined`, which is worse than refusing the surface. A typed builtin can expose a
/// broader interface than its native materializer, so named imports are checked against the
/// materialized subset here as well. Keep this gate beside native graph construction until each
/// accepted builtin has an explicit native materializer.
fn validate_native_node_builtins(graph: &ModuleGraph) -> Result<(), NativeError> {
    for node in graph.modules() {
        let ModuleSource::NodeBuiltin(builtin) = node.source else {
            continue;
        };
        if native_builtin_body(builtin).is_none() {
            return Err(NativeError::UnsupportedNodeBuiltin {
                specifier: builtin.specifier(),
            });
        }
    }
    for importer in graph.modules() {
        for import in &importer.program.imports {
            let Some(dependency) = importer
                .dependencies
                .iter()
                .find(|dependency| dependency.specifier == import.source)
            else {
                continue;
            };
            let Some(provider) = graph.module(&dependency.target) else {
                continue;
            };
            let ModuleSource::NodeBuiltin(builtin) = provider.source else {
                continue;
            };
            if import.imported != kiln_ir::NAMESPACE_IMPORT
                && !native_builtin_supports_export(builtin, &import.imported)
            {
                return Err(NativeError::UnsupportedNodeBuiltinExport {
                    specifier: builtin.specifier(),
                    export_name: import.imported.clone(),
                });
            }
        }
    }
    Ok(())
}

fn native_builtin_supports_export(
    builtin: kiln_module_resolver::NodeBuiltin,
    export_name: &str,
) -> bool {
    native_builtin_supported_exports(builtin).contains(&export_name)
}

/// Names whose native materializer carries an implementation rather than an alignment slot.
///
/// `kiln-node-abi` describes the Node-visible module surface shared by every execution mode. A
/// native materializer may deliberately implement a smaller subset, so using that full table as
/// the support predicate turns an `undefined` alignment slot into a claimed implementation. Keep
/// the subset explicit until the corresponding C methods exist; the slot invariant below proves
/// that each body still follows the authoritative full-table order.
fn native_builtin_supported_exports(
    builtin: kiln_module_resolver::NodeBuiltin,
) -> &'static [&'static str] {
    use kiln_module_resolver::NodeBuiltin;

    match builtin {
        NodeBuiltin::Events => &["EventEmitter", "default"],
        NodeBuiltin::Net => &["connect", "createConnection", "createServer", "default"],
        NodeBuiltin::Http => &[
            "Server",
            "ServerResponse",
            "createServer",
            "default",
            "get",
            "request",
        ],
        NodeBuiltin::Util => &[
            // `TextDecoder` is declared beside it in the ABI and is deliberately absent here:
            // the C runtime has an encoder builtin and no decoder, so the decoder holds an
            // alignment slot and is refused as an import rather than answered with `undefined`.
            "TextEncoder",
            "debuglog",
            "default",
            "deprecate",
            "format",
            "inherits",
            "inspect",
        ],
        NodeBuiltin::DiagnosticsChannel => {
            &["channel", "default", "hasSubscribers", "tracingChannel"]
        }
        NodeBuiltin::Https => &["createServer", "default"],
        NodeBuiltin::Http2 => &["createSecureServer", "createServer", "default"],
        NodeBuiltin::Dns => &["default", "lookup"],
        NodeBuiltin::Os => &["default", "hostname", "networkInterfaces"],
        NodeBuiltin::Tty => &["default", "isatty"],
        NodeBuiltin::Stream => &[
            "Readable",
            "Writable",
            "addAbortSignal",
            "default",
            "finished",
        ],
        NodeBuiltin::PerfHooks => &["default", "performance"],
        NodeBuiltin::FsPromises => &["readFile", "stat", "writeFile"],
        NodeBuiltin::Fs => &["default", "existsSync", "promises"],
        NodeBuiltin::Path => &["default", "isAbsolute", "join", "resolve", "sep"],
        NodeBuiltin::Assert => &["default", "ok", "strict"],
        NodeBuiltin::WorkerThreads => &["Worker", "default"],
        NodeBuiltin::Module => &["createRequire", "default"],
        NodeBuiltin::QueryString => &["decode", "default", "parse"],
        NodeBuiltin::Url => &["URL", "default", "fileURLToPath", "parse", "pathToFileURL"],
        NodeBuiltin::Buffer => &["Buffer", "constants", "default"],
        NodeBuiltin::AsyncHooks => &["AsyncResource", "default"],
        NodeBuiltin::Crypto => &["createHash", "default", "randomUUID"],
        _ => &[],
    }
}

fn emit_native_graph_module(
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &NativeRealmPolicy,
) -> Result<kiln_backend_c::CModule, NativeError> {
    match node.source {
        ModuleSource::NodeBuiltin(builtin) => {
            let materializer =
                native_builtin_body(builtin).ok_or(NativeError::UnsupportedNodeBuiltin {
                    specifier: builtin.specifier(),
                })?;
            let (program, body) =
                native_builtin_runtime_module(&node.program, builtin, materializer)?;
            Ok(emit_runtime_module_with_logical_path(
                &program,
                logical_path,
                &body,
            )?)
        }
        ModuleSource::CommonJs => Ok(emit_commonjs_module_with_logical_path_and_policy(
            &node.program,
            logical_path,
            policy.emission(),
        )?),
        ModuleSource::Json => {
            let source = node.data.as_deref().ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::Source {
                    path: node.path.clone(),
                    message: "JSON module has no captured source text".to_owned(),
                })
            })?;
            let body = format!(
                "int status = kiln_c99_json_document({}, (size_t){}, &exports[0]);\nif (status != KILN_C99_OK) return status;",
                native_c_string(source),
                source.len()
            );
            Ok(emit_runtime_module_with_logical_path(
                &node.program,
                logical_path,
                &body,
            )?)
        }
        ModuleSource::Source => Ok(emit_module_with_logical_path_and_policy(
            &node.program,
            logical_path,
            policy.emission(),
        )?),
    }
}

fn native_builtin_body(builtin: kiln_module_resolver::NodeBuiltin) -> Option<&'static str> {
    use kiln_module_resolver::NodeBuiltin;
    match builtin {
        NodeBuiltin::Events => Some(
            r#"kiln_value constructor = kiln_c99_method(KILN_C99_METHOD_NODE_EVENT_EMITTER);
exports[0] = constructor;
exports[1] = constructor;"#,
        ),
        NodeBuiltin::Net => Some(
            r#"kiln_value connect = kiln_c99_method(KILN_C99_METHOD_NODE_NET_CONNECT);
kiln_value create_server = kiln_c99_method(KILN_C99_METHOD_NODE_NET_CREATE_SERVER);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "connect", 7, connect, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "createConnection", 16, connect, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "createServer", 12, create_server, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = connect;
exports[1] = connect;
exports[2] = create_server;
exports[3] = namespace;"#,
        ),
        // Slots are positions in `node_builtin_export_names("http")`, which lives in
        // kiln-interpreter and is shared with the interpreter and `process.getBuiltinModule`.
        // The indices below are therefore a cross-crate coupling, and they silently rotted once
        // already: this body was written against a six-name list and the table later grew
        // `IncomingMessage`, `METHODS` and `ServerResponse` at the front, which shifted every
        // slot. Nothing failed to compile. `default` landed on `request`, so `typeof http` was
        // "function" instead of "object", and `createServer` landed on `get`, so the first real
        // server refused with "a function is not a request options object" - a wrong answer
        // rather than a refusal, which is the class regress/r07 exists to catch.
        //
        // `native_builtin_slots_match_the_shared_export_table` now pins this, so the next
        // addition to the table fails a test here instead of miscompiling a server. The three
        // Remaining gated names hold `undefined` because `native_builtin_supports_export`
        // refuses them as named imports; they are slots to keep alignment, not exports.
        NodeBuiltin::Http => Some(
            r#"kiln_value server = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_SERVER);
kiln_value server_response = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE);
kiln_value create_server = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER);
kiln_value get = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_GET);
kiln_value request = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_REQUEST);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "Server", 6, server, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "ServerResponse", 14, server_response, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "createServer", 12, create_server, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "get", 3, get, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "request", 7, request, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = kiln_c99_undefined();
exports[1] = kiln_c99_undefined();
exports[2] = server;
exports[3] = server_response;
exports[4] = create_server;
exports[5] = namespace;
exports[6] = get;
exports[7] = request;"#,
        ),
        NodeBuiltin::Util => Some(
            // Slot 0 is `TextDecoder`, which this runtime has no builtin for. It holds
            // `undefined` to keep the slots aligned with the shared table and is refused as a
            // named import by `native_builtin_supports_export`; it is a slot, not an export.
            r#"kiln_value text_encoder = kiln_c99_builtin(KILN_C99_BUILTIN_TEXT_ENCODER);
kiln_value debuglog = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_DEBUGLOG);
kiln_value deprecate = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_DEPRECATE);
kiln_value format = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_FORMAT);
kiln_value inherits = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_INHERITS);
kiln_value inspect = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_INSPECT);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "debuglog", 8, debuglog, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "deprecate", 9, deprecate, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "format", 6, format, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "inherits", 8, inherits, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "inspect", 7, inspect, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "TextEncoder", 11, text_encoder, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = kiln_c99_undefined();
exports[1] = text_encoder;
exports[2] = debuglog;
exports[3] = namespace;
exports[4] = deprecate;
exports[5] = format;
exports[6] = inherits;
exports[7] = inspect;"#,
        ),
        NodeBuiltin::DiagnosticsChannel => Some(
            r#"kiln_value channel = kiln_c99_method(KILN_C99_METHOD_NODE_DIAGNOSTICS_CHANNEL);
kiln_value tracing_channel = kiln_c99_method(KILN_C99_METHOD_NODE_DIAGNOSTICS_TRACING_CHANNEL);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "channel", 7, channel, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "hasSubscribers", 14, kiln_c99_boolean(0), 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "tracingChannel", 14, tracing_channel, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = channel;
exports[1] = namespace;
exports[2] = kiln_c99_boolean(0);
exports[3] = tracing_channel;"#,
        ),
        NodeBuiltin::Https => Some(
            r#"kiln_value create_server = kiln_c99_method(KILN_C99_METHOD_NODE_HTTPS_CREATE_SERVER);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "createServer", 12, create_server, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = create_server;
exports[1] = namespace;"#,
        ),
        NodeBuiltin::Http2 => Some(
            r#"kiln_value create_secure = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP2_CREATE_SECURE_SERVER);
kiln_value create_server = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP2_CREATE_SERVER);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "createSecureServer", 18, create_secure, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "createServer", 12, create_server, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = create_secure;
exports[1] = create_server;
exports[2] = namespace;"#,
        ),
        NodeBuiltin::Dns => Some(
            r#"kiln_value lookup = kiln_c99_method(KILN_C99_METHOD_NODE_DNS_LOOKUP);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "lookup", 6, lookup, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = namespace;
exports[1] = lookup;"#,
        ),
        NodeBuiltin::Os => Some(
            r#"kiln_value hostname = kiln_c99_method(KILN_C99_METHOD_NODE_OS_HOSTNAME);
kiln_value network_interfaces = kiln_c99_method(KILN_C99_METHOD_NODE_OS_NETWORK_INTERFACES);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "hostname", 8, hostname, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "networkInterfaces", 17, network_interfaces, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = kiln_c99_undefined();
exports[1] = kiln_c99_undefined();
exports[2] = kiln_c99_undefined();
exports[3] = kiln_c99_undefined();
exports[4] = namespace;
exports[5] = kiln_c99_undefined();
exports[6] = kiln_c99_undefined();
exports[7] = kiln_c99_undefined();
exports[8] = kiln_c99_undefined();
exports[9] = hostname;
exports[10] = kiln_c99_undefined();
exports[11] = kiln_c99_undefined();
exports[12] = network_interfaces;
exports[13] = kiln_c99_undefined();
exports[14] = kiln_c99_undefined();
exports[15] = kiln_c99_undefined();
exports[16] = kiln_c99_undefined();
exports[17] = kiln_c99_undefined();
exports[18] = kiln_c99_undefined();
exports[19] = kiln_c99_undefined();
exports[20] = kiln_c99_undefined();"#,
        ),
        NodeBuiltin::Tty => Some(
            r#"kiln_value isatty = kiln_c99_method(KILN_C99_METHOD_NODE_TTY_ISATTY);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "isatty", 6, isatty, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = namespace;
exports[1] = isatty;"#,
        ),
        NodeBuiltin::Stream => Some(
            r#"kiln_value readable = kiln_c99_method(KILN_C99_METHOD_NODE_READABLE);
kiln_value writable = kiln_c99_method(KILN_C99_METHOD_NODE_WRITABLE);
kiln_value add_abort_signal = kiln_c99_method(KILN_C99_METHOD_NODE_STREAM_ADD_ABORT_SIGNAL);
kiln_value finished = kiln_c99_method(KILN_C99_METHOD_NODE_STREAM_FINISHED);
kiln_value stream = kiln_c99_method(KILN_C99_METHOD_NODE_STREAM);
int status = kiln_c99_put(host, stream, "Readable", 8, readable, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, stream, "Writable", 8, writable, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, stream, "addAbortSignal", 14, add_abort_signal, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, stream, "finished", 8, finished, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = readable;
exports[1] = writable;
exports[2] = add_abort_signal;
exports[3] = stream;
exports[4] = finished;"#,
        ),
        NodeBuiltin::PerfHooks => Some(
            r#"kiln_value performance;
int status = kiln_c99_new_object(&performance);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, performance, "now", 3, kiln_c99_method(KILN_C99_METHOD_NODE_PERFORMANCE_NOW), 0, NULL);
if (status != KILN_C99_OK) return status;
kiln_value namespace;
status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "performance", 11, performance, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = namespace;
exports[1] = performance;"#,
        ),
        NodeBuiltin::FsPromises => Some(
            r#"kiln_value read_file = kiln_c99_method(KILN_C99_METHOD_NODE_FS_READ_FILE);
kiln_value stat = kiln_c99_method(KILN_C99_METHOD_NODE_FS_STAT);
kiln_value write_file = kiln_c99_method(KILN_C99_METHOD_NODE_FS_WRITE_FILE);
exports[0] = read_file;
exports[1] = stat;
exports[2] = write_file;"#,
        ),
        NodeBuiltin::Fs => Some(
            r#"kiln_value exists_sync = kiln_c99_method(KILN_C99_METHOD_NODE_FS_EXISTS_SYNC);
kiln_value promises;
int status = kiln_c99_new_object(&promises);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, promises, "readFile", 8, kiln_c99_method(KILN_C99_METHOD_NODE_FS_READ_FILE), 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, promises, "stat", 4, kiln_c99_method(KILN_C99_METHOD_NODE_FS_STAT), 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, promises, "writeFile", 9, kiln_c99_method(KILN_C99_METHOD_NODE_FS_WRITE_FILE), 0, NULL);
if (status != KILN_C99_OK) return status;
kiln_value namespace;
status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "existsSync", 10, exists_sync, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "promises", 8, promises, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = kiln_c99_undefined();
exports[1] = kiln_c99_undefined();
exports[2] = kiln_c99_undefined();
exports[3] = kiln_c99_undefined();
exports[4] = namespace;
exports[5] = exists_sync;
exports[6] = kiln_c99_undefined();
exports[7] = kiln_c99_undefined();
exports[8] = kiln_c99_undefined();
exports[9] = promises;
exports[10] = kiln_c99_undefined();
exports[11] = kiln_c99_undefined();
exports[12] = kiln_c99_undefined();
exports[13] = kiln_c99_undefined();
exports[14] = kiln_c99_undefined();
exports[15] = kiln_c99_undefined();
exports[16] = kiln_c99_undefined();
exports[17] = kiln_c99_undefined();
exports[18] = kiln_c99_undefined();
exports[19] = kiln_c99_undefined();"#,
        ),
        NodeBuiltin::Path => Some(
            r#"kiln_value is_absolute = kiln_c99_method(KILN_C99_METHOD_NODE_PATH_IS_ABSOLUTE);
kiln_value join = kiln_c99_method(KILN_C99_METHOD_NODE_PATH_JOIN);
kiln_value resolve = kiln_c99_method(KILN_C99_METHOD_NODE_PATH_RESOLVE);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "isAbsolute", 10, is_absolute, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "join", 4, join, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "resolve", 7, resolve, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "sep", 3, kiln_c99_copy_text("/", 1), 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = kiln_c99_undefined();
exports[1] = namespace;
exports[2] = kiln_c99_undefined();
exports[3] = kiln_c99_undefined();
exports[4] = kiln_c99_undefined();
exports[5] = kiln_c99_undefined();
exports[6] = is_absolute;
exports[7] = join;
exports[8] = kiln_c99_undefined();
exports[9] = kiln_c99_undefined();
exports[10] = kiln_c99_undefined();
exports[11] = kiln_c99_undefined();
exports[12] = resolve;
exports[13] = kiln_c99_copy_text("/", 1);"#,
        ),
        NodeBuiltin::Assert => Some(
            r#"kiln_value assert = kiln_c99_method(KILN_C99_METHOD_NODE_ASSERT_OK);
exports[0] = assert;
exports[1] = assert;
exports[2] = assert;"#,
        ),
        NodeBuiltin::WorkerThreads => Some(
            r#"kiln_value worker = kiln_c99_method(KILN_C99_METHOD_NODE_WORKER_CONSTRUCTOR);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "Worker", 6, worker, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = worker;
exports[1] = namespace;"#,
        ),
        NodeBuiltin::Module => Some(
            r#"kiln_value create_require = kiln_c99_method(KILN_C99_METHOD_NODE_MODULE_CREATE_REQUIRE);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "createRequire", 13, create_require, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = create_require;
exports[1] = namespace;"#,
        ),
        NodeBuiltin::QueryString => Some(
            r#"kiln_value parse = kiln_c99_method(KILN_C99_METHOD_NODE_QUERYSTRING_PARSE);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "decode", 6, parse, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "parse", 5, parse, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = parse;
exports[1] = namespace;
exports[2] = parse;"#,
        ),
        NodeBuiltin::Url => Some(
            r#"kiln_value constructor = kiln_c99_builtin(KILN_C99_BUILTIN_URL);
kiln_value file_url_to_path = kiln_c99_method(KILN_C99_METHOD_NODE_URL_FILE_URL_TO_PATH);
kiln_value parse = kiln_c99_method(KILN_C99_METHOD_NODE_URL_PARSE);
kiln_value path_to_file_url = kiln_c99_method(KILN_C99_METHOD_NODE_URL_PATH_TO_FILE_URL);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "URL", 3, constructor, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "fileURLToPath", 13, file_url_to_path, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "parse", 5, parse, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "pathToFileURL", 13, path_to_file_url, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = constructor;
exports[1] = namespace;
exports[2] = file_url_to_path;
exports[3] = parse;
exports[4] = path_to_file_url;"#,
        ),
        NodeBuiltin::Buffer => Some(
            r#"kiln_value constructor = kiln_c99_builtin(KILN_C99_BUILTIN_BUFFER);
kiln_value constants;
int status = kiln_c99_new_object(&constants);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, constants, "MAX_STRING_LENGTH", 17, kiln_c99_number(536870888.0), 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, constructor, "constants", 9, constants, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = constructor;
exports[1] = constants;
exports[2] = constructor;"#,
        ),
        NodeBuiltin::AsyncHooks => Some(
            // Slot 0 is `AsyncLocalStorage`. The interpreter has one; this runtime does not,
            // because a store has to be captured when work is queued and restored when it runs,
            // and that is a change to the C job queue rather than one more builtin. It holds an
            // alignment slot and is refused as a named import rather than answered `undefined`.
            r#"kiln_value resource = kiln_c99_method(KILN_C99_METHOD_NODE_ASYNC_RESOURCE);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "AsyncResource", 13, resource, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = kiln_c99_undefined();
exports[1] = resource;
exports[2] = namespace;"#,
        ),
        NodeBuiltin::Crypto => Some(
            r#"kiln_value create_hash = kiln_c99_method(KILN_C99_METHOD_NODE_CRYPTO_CREATE_HASH);
kiln_value random_uuid = kiln_c99_method(KILN_C99_METHOD_NODE_CRYPTO_RANDOM_UUID);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "createHash", 10, create_hash, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "randomUUID", 10, random_uuid, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = create_hash;
exports[1] = namespace;
exports[2] = random_uuid;"#,
        ),
        _ => None,
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

fn cache_identity(
    compiler: impl Into<String>,
    backend: Backend,
    abi: &str,
    target: &str,
    profile: CacheProfile,
) -> Digest {
    key(
        ArtifactKind::GeneratedC,
        compiler,
        backend,
        abi,
        target,
        profile,
        Digest::hash(b"identity representative"),
        [],
    )
    .identity_digest()
}

fn module_abi_identity() -> String {
    // Generated modules carry the private value runtime in their translation unit today. Any
    // change to that source therefore changes the object ABI even when the public scalar C ABI
    // does not. Pin the generator revision here so a stale object can never be linked beside a
    // freshly generated sibling merely because both still expose C ABI v1.
    format!("host-{HOST_ABI_VERSION};c-{C_ABI_VERSION};generator-{C_GENERATOR_VERSION}")
}

fn runtime_abi_identity() -> String {
    format!(
        "{};runtime-{NATIVE_RUNTIME_VERSION};async-{PRIVATE_ASYNC_ABI_VERSION}",
        module_abi_identity()
    )
}

fn shared_embedding_abi_identity() -> String {
    format!(
        "{};module-api-{EMBEDDING_MODULE_API_VERSION}",
        runtime_abi_identity()
    )
}

fn native_number_abi_identity() -> String {
    format!("kiln-native-number-provider-{PROVIDER_ABI_VERSION}")
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct LogicalModulePath(String);

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommonJsFilename(String);

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommonJsDirname(String);

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommonJsModulePaths {
    filename: CommonJsFilename,
    dirname: CommonJsDirname,
    diagnostic: LogicalModulePath,
}

impl LogicalModulePath {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl CommonJsFilename {
    fn new(path: &Path) -> Result<Self, NativeError> {
        if !path.is_absolute() {
            return Err(NativeError::Emit(EmitError::InvalidProgram {
                message: format!("CommonJS module path {} is not absolute", path.display()),
            }));
        }
        Ok(Self(path.to_string_lossy().into_owned()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl CommonJsDirname {
    fn new(path: &Path) -> Result<Self, NativeError> {
        let dirname = path.parent().ok_or_else(|| {
            NativeError::Emit(EmitError::InvalidProgram {
                message: format!("CommonJS module path {} has no parent", path.display()),
            })
        })?;
        Ok(Self(dirname.to_string_lossy().into_owned()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl CommonJsModulePaths {
    fn new(root: &Path, path: &Path) -> Result<Self, NativeError> {
        Ok(Self {
            filename: CommonJsFilename::new(path)?,
            dirname: CommonJsDirname::new(path)?,
            diagnostic: LogicalModulePath(logical_module_path(root, path)?),
        })
    }
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

/// Derive the public graph wrapper identity from topology, ordered module bodies, and every
/// absolute CommonJS runtime path embedded by the wrapper. Private module symbols intentionally
/// remain topology-based, but a public wrapper must not collide with another DSO containing the
/// same logical graph and bytecode but different observable `__filename`/`__dirname` values.
fn graph_shared_public_digest(
    topology: Digest,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
) -> Result<Digest, NativeError> {
    let mut bytes = topology.to_hex().into_bytes();
    bytes.push(0);
    for (_, module) in modules {
        let digest = Digest::hash(module.source.as_bytes());
        bytes.extend_from_slice(digest.to_hex().as_bytes());
        bytes.push(0);
    }
    if modules
        .iter()
        .any(|(node, _)| node.source == ModuleSource::CommonJs)
    {
        bytes.extend_from_slice(b"commonjs-runtime-paths-v1\0");
        for (node, _) in modules {
            if node.source != ModuleSource::CommonJs {
                continue;
            }
            let filename = CommonJsFilename::new(&node.path)?;
            bytes.extend_from_slice(&(filename.as_bytes().len() as u64).to_le_bytes());
            bytes.extend_from_slice(filename.as_bytes());
        }
    }
    Ok(Digest::hash(&bytes))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeGraphImport {
    Internal,
    Binding {
        provider_index: usize,
        export_index: usize,
    },
    Namespace {
        provider_index: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum NativeNamespaceValue {
    Binding {
        provider_index: usize,
        export_index: usize,
    },
    Namespace {
        provider_index: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeNamespaceEntry {
    name: String,
    value: NativeNamespaceValue,
}

/// Resolve one canonical binding slot from the graph's export table.
///
/// Raw provider export tables are insufficient here: `export *` deliberately has only one
/// synthetic entry, while the graph linker resolves each forwarded name to its canonical origin.
fn graph_export_slot(
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    indices: &std::collections::HashMap<PathBuf, usize>,
    importer: &Path,
    origin: &Path,
    local_slot: u32,
    exported_name: &str,
) -> Result<(usize, usize), NativeError> {
    let provider_index = *indices.get(origin).ok_or_else(|| {
        NativeError::Graph(ModuleGraphError::InternalMissingModule {
            path: origin.to_path_buf(),
        })
    })?;
    /* A CommonJS module publishes its entire `module.exports` projection in slot zero. Named
     * re-export aliases still resolve to that same origin even though their final spelling is no
     * longer `default`. */
    if modules[provider_index].0.source == ModuleSource::CommonJs {
        return Ok((provider_index, 0));
    }
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
                importer: importer.to_path_buf(),
                provider: origin.to_path_buf(),
                name: exported_name.to_owned(),
            })
        })?;
    Ok((provider_index, export_index))
}

/// Resolve one module import to either its concrete export-array slot or one graph namespace.
///
/// The synthetic namespace import used solely to carry a star table remains internal. A visible
/// namespace is a single snapshot object built after its provider finishes, then shared by every
/// importer during this graph invocation.
fn graph_import_value(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    indices: &std::collections::HashMap<PathBuf, usize>,
    node: &kiln_module_graph::ModuleNode,
    import: &kiln_ir::ImportBinding,
) -> Result<NativeGraphImport, NativeError> {
    /* CommonJS's five wrapper bindings are supplied by the graph launcher, not by another
     * module node. Keeping this sentinel out of ordinary dependency resolution is what lets the
     * launcher preserve one cache identity for `exports`/`module.exports` and attach a require
     * function whose target table is scoped to this importer. */
    if node.source == ModuleSource::CommonJs && import.source == "*commonjs-wrapper*" {
        return Ok(NativeGraphImport::Internal);
    }
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
            return Ok(NativeGraphImport::Internal);
        }
        let provider_index = *indices.get(&dependency.target).ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::InternalMissingModule {
                path: dependency.target.clone(),
            })
        })?;
        let importer_index = *indices.get(&node.path).ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::InternalMissingModule {
                path: node.path.clone(),
            })
        })?;
        if provider_index >= importer_index {
            return Err(NativeError::Emit(EmitError::Unsupported {
                feature: "self or cyclic namespace imports in native module graphs".to_owned(),
            }));
        }
        return Ok(NativeGraphImport::Namespace { provider_index });
    }

    let (origin, local_slot) = match graph.resolved_export(&dependency.target, &import.imported) {
        Some(ResolvedExport::Local { module, local_slot }) => (module, *local_slot),
        Some(ResolvedExport::Namespace { module }) => {
            let provider_index = *indices.get(module).ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::InternalMissingModule {
                    path: module.clone(),
                })
            })?;
            let importer_index = *indices.get(&node.path).ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::InternalMissingModule {
                    path: node.path.clone(),
                })
            })?;
            if provider_index >= importer_index {
                return Err(NativeError::Emit(EmitError::Unsupported {
                    feature: "self or cyclic namespace re-exports in native module graphs"
                        .to_owned(),
                }));
            }
            return Ok(NativeGraphImport::Namespace { provider_index });
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
    let (provider_index, export_index) = graph_export_slot(
        modules,
        indices,
        &node.path,
        origin,
        local_slot,
        &import.imported,
    )?;
    /* The launcher is straight-line C: it calls each module in order and takes an importer's
     * values out of the export array the provider already returned. A cycle can make the origin
     * of a name come *after* the module that imports it, and there is no array to read then --
     * the generated code would read one that has not been filled. The interpreter answers that
     * with a binding cell it fills in later; this backend has no such thing, so it refuses by
     * name. A cyclic graph whose back edges carry no binding still compiles, because nothing in
     * it reads forward. */
    let importer_index = *indices.get(&node.path).ok_or_else(|| {
        NativeError::Graph(ModuleGraphError::InternalMissingModule {
            path: node.path.clone(),
        })
    })?;
    if provider_index >= importer_index {
        return Err(NativeError::Emit(EmitError::Unsupported {
            feature: format!(
                "`{}`, imported across a module cycle before `{}` has run, in native module \
                 graphs",
                import.imported,
                origin.display()
            ),
        }));
    }
    Ok(NativeGraphImport::Binding {
        provider_index,
        export_index,
    })
}

/// The sorted snapshot contents each graph module would expose through `import * as`.
fn graph_namespace_plans(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    indices: &std::collections::HashMap<PathBuf, usize>,
) -> Result<Vec<Option<Vec<NativeNamespaceEntry>>>, NativeError> {
    let mut required = vec![false; modules.len()];
    for (node, _) in modules {
        for import in &node.program.imports {
            if let NativeGraphImport::Namespace { provider_index } =
                graph_import_value(graph, modules, indices, node, import)?
            {
                required[provider_index] = true;
            }
        }
    }

    let mut plans = vec![None; modules.len()];
    /* Dependencies precede importers. Walking backwards lets a required barrel namespace mark
     * the namespace it re-exports before that provider is planned. */
    for module_index in (0..modules.len()).rev() {
        if !required[module_index] {
            continue;
        }
        let node = modules[module_index].0;
        let names = graph.resolved_export_names(&node.path).ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::InternalMissingModule {
                path: node.path.clone(),
            })
        })?;
        let entries = names
            .into_iter()
            .map(|name| {
                if name.starts_with("@@") {
                    return Err(NativeError::Emit(EmitError::Unsupported {
                        feature: "@@-prefixed exports in native module namespaces".to_owned(),
                    }));
                }
                let value = match graph.resolved_export(&node.path, &name) {
                    Some(ResolvedExport::Local { module, local_slot }) => {
                        let (provider_index, export_index) = graph_export_slot(
                            modules,
                            indices,
                            &node.path,
                            module,
                            *local_slot,
                            &name,
                        )?;
                        NativeNamespaceValue::Binding {
                            provider_index,
                            export_index,
                        }
                    }
                    Some(ResolvedExport::Namespace { module }) => {
                        let provider_index = *indices.get(module).ok_or_else(|| {
                            NativeError::Graph(ModuleGraphError::InternalMissingModule {
                                path: module.clone(),
                            })
                        })?;
                        if provider_index >= module_index {
                            return Err(NativeError::Emit(EmitError::Unsupported {
                                feature:
                                    "self or cyclic namespace re-exports in native module graphs"
                                        .to_owned(),
                            }));
                        }
                        required[provider_index] = true;
                        NativeNamespaceValue::Namespace { provider_index }
                    }
                    Some(ResolvedExport::Ambiguous) | None => {
                        return Err(NativeError::Graph(ModuleGraphError::MissingExport {
                            importer: node.path.clone(),
                            provider: node.path.clone(),
                            name: name.clone(),
                        }));
                    }
                };
                Ok(NativeNamespaceEntry { name, value })
            })
            .collect::<Result<Vec<_>, NativeError>>()?;
        plans[module_index] = Some(entries);
    }
    Ok(plans)
}

fn native_graph_export_count(node: &kiln_module_graph::ModuleNode) -> usize {
    if node.source == ModuleSource::CommonJs {
        1
    } else if matches!(node.source, ModuleSource::NodeBuiltin(_)) {
        node.program.exports.len() + 1
    } else {
        node.program.exports.len()
    }
}

/// The slot whose value CommonJS receives from `require()` for a typed builtin.
///
/// Typed builtins reserve one private slot after their public ESM exports. That slot is the only
/// CommonJS projection; loaders never guess that `default` or export zero happens to be suitable.
fn native_commonjs_projection_index(node: &kiln_module_graph::ModuleNode) -> Option<usize> {
    match node.source {
        ModuleSource::Json => Some(0),
        ModuleSource::NodeBuiltin(_) => Some(node.program.exports.len()),
        ModuleSource::Source | ModuleSource::CommonJs => None,
    }
}

fn native_builtin_runtime_module(
    public: &ModuleProgram,
    builtin: kiln_module_resolver::NodeBuiltin,
    materializer: &str,
) -> Result<(ModuleProgram, String), NativeError> {
    let abi = kiln_node_abi::builtin_abi(builtin.specifier()).ok_or(
        NativeError::UnsupportedNodeBuiltin {
            specifier: builtin.specifier(),
        },
    )?;
    let mut program = public.clone();
    let private_slot = program.exports.len();
    let local_slot = program.program.local_count;
    program.program.local_count = local_slot.checked_add(1).ok_or_else(|| {
        NativeError::Graph(ModuleGraphError::Source {
            path: std::path::PathBuf::from(builtin.specifier()),
            message: "builtin CommonJS projection exhausted local slots".to_owned(),
        })
    })?;
    program.exports.push(ExportBinding {
        exported: "*private*commonjs".to_owned(),
        local_slot,
    });

    let export_index = |name: &str| {
        public
            .exports
            .iter()
            .position(|binding| binding.exported == name)
    };
    let mut body = String::from(materializer);
    body.push('\n');
    match abi.commonjs {
        kiln_node_abi::CommonJsProjection::Namespace => {
            writeln!(body, "kiln_value commonjs_namespace;").unwrap();
            writeln!(
                body,
                "int commonjs_status = kiln_c99_new_object(&commonjs_namespace);"
            )
            .unwrap();
            writeln!(
                body,
                "if (commonjs_status != KILN_C99_OK) return commonjs_status;"
            )
            .unwrap();
            for export in abi.exports {
                let index =
                    export_index(export).ok_or(NativeError::UnsupportedNodeBuiltinExport {
                        specifier: builtin.specifier(),
                        export_name: (*export).to_owned(),
                    })?;
                writeln!(body, "commonjs_status = kiln_c99_put(host, commonjs_namespace, {}, {}, exports[{index}], 0, NULL);", native_c_string(export), export.len()).unwrap();
                writeln!(
                    body,
                    "if (commonjs_status != KILN_C99_OK) return commonjs_status;"
                )
                .unwrap();
            }
            writeln!(body, "exports[{private_slot}] = commonjs_namespace;").unwrap();
        }
        kiln_node_abi::CommonJsProjection::Callable { export, properties } => {
            let index = export_index(export).ok_or(NativeError::UnsupportedNodeBuiltinExport {
                specifier: builtin.specifier(),
                export_name: export.to_owned(),
            })?;
            writeln!(body, "exports[{private_slot}] = exports[{index}];").unwrap();
            if !properties.is_empty() {
                writeln!(body, "int commonjs_status;").unwrap();
            }
            for (property, target) in properties {
                let target_index =
                    export_index(target).ok_or(NativeError::UnsupportedNodeBuiltinExport {
                        specifier: builtin.specifier(),
                        export_name: (*target).to_owned(),
                    })?;
                writeln!(body, "commonjs_status = kiln_c99_put(host, exports[{private_slot}], {}, {}, exports[{target_index}], 0, NULL);", native_c_string(property), property.len()).unwrap();
                writeln!(
                    body,
                    "if (commonjs_status != KILN_C99_OK) return commonjs_status;"
                )
                .unwrap();
            }
        }
    }
    Ok((program, body))
}

fn native_c_string(value: &str) -> String {
    let mut output = String::from("\"");
    for byte in value.bytes() {
        match byte {
            b'\"' => output.push_str("\\\""),
            b'\\' => output.push_str("\\\\"),
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            0x20..=0x7e => output.push(char::from(byte)),
            _ => write!(output, "\\x{byte:02X}\"\"").unwrap(),
        }
    }
    output.push('\"');
    output
}

fn write_graph_commonjs_realm_setup(
    output: &mut String,
    index: usize,
    module_symbol: &str,
    filename: &CommonJsFilename,
    dirname: &CommonJsDirname,
    diagnostic: &LogicalModulePath,
) {
    writeln!(
        output,
        "    status = {module_symbol}_commonjs_wrapper(kiln_imports_{index}, (size_t)5, {}, (size_t){}, {}, (size_t){}, kiln_commonjs_require_{index}, NULL);\n    if (status != 0) {{ *result = kiln_graph_diagnostic(\"commonjs-wrapper\", {}); return status; }}\n    kiln_exports_{index}[0] = kiln_imports_{index}[0];\n    kiln_commonjs_{index}.module = {module_symbol};\n    kiln_commonjs_{index}.host = host;\n    kiln_commonjs_{index}.imports = kiln_imports_{index};\n    kiln_commonjs_{index}.import_count = (size_t)5;\n    kiln_commonjs_{index}.exports = kiln_exports_{index};\n    kiln_commonjs_{index}.state = 0;",
        native_c_string(filename.as_str()),
        filename.as_str().len(),
        native_c_string(dirname.as_str()),
        dirname.as_str().len(),
        native_c_string(diagnostic.as_str()),
    )
    .unwrap();
}

fn write_graph_commonjs_shared_setup(
    output: &mut String,
    index: usize,
    module_symbol: &str,
    filename: &CommonJsFilename,
    dirname: &CommonJsDirname,
) {
    writeln!(
        output,
        "    status = {module_symbol}_commonjs_wrapper(import_arrays[{index}], (size_t)5, {}, (size_t){}, {}, (size_t){}, kiln_commonjs_require_{index}, &commonjs_graph);\n    if (status != KILN_C99_OK) goto cleanup;\n    export_arrays[{index}][0] = import_arrays[{index}][0];\n    commonjs_modules[{index}] = (kiln_commonjs_context){{ {module_symbol}, proxy_host, import_arrays[{index}], (size_t)5, export_arrays[{index}], 0 }};",
        native_c_string(filename.as_str()),
        filename.as_str().len(),
        native_c_string(dirname.as_str()),
        dirname.as_str().len(),
    )
    .unwrap();
}

fn write_graph_namespace_build(
    output: &mut String,
    module_index: usize,
    module_symbol: &str,
    entries: &[NativeNamespaceEntry],
    shared: bool,
) {
    let namespaces = if shared {
        "namespaces"
    } else {
        "kiln_namespaces"
    };
    if entries.is_empty() {
        writeln!(
            output,
            "    status = {module_symbol}_namespace_wrapper(&{namespaces}[{module_index}], NULL, NULL, NULL, (size_t)0);"
        )
        .unwrap();
        return;
    }
    let names = entries
        .iter()
        .map(|entry| native_c_string(&entry.name))
        .collect::<Vec<_>>()
        .join(", ");
    let lengths = entries
        .iter()
        .map(|entry| format!("(size_t){}", entry.name.len()))
        .collect::<Vec<_>>()
        .join(", ");
    let values = entries
        .iter()
        .map(|entry| match entry.value {
            NativeNamespaceValue::Binding {
                provider_index,
                export_index,
            } if shared => format!("export_arrays[{provider_index}][{export_index}]"),
            NativeNamespaceValue::Binding {
                provider_index,
                export_index,
            } => format!("kiln_exports_{provider_index}[{export_index}]"),
            NativeNamespaceValue::Namespace { provider_index } => {
                format!("{namespaces}[{provider_index}]")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        output,
        "    status = {module_symbol}_namespace_wrapper(&{namespaces}[{module_index}], (const char *const[]){{ {names} }}, (const size_t[]){{ {lengths} }}, (const kiln_value[]){{ {values} }}, (size_t){});",
        entries.len()
    )
    .unwrap();
}

/// The graph launcher's root function (ADR 0041): the export, import and namespace arrays it
/// links with, and the two result slots `main` reads after the realm is destroyed.
fn render_graph_roots(
    output: &mut String,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
) {
    output.push_str(
        "static void kiln_graph_gc_roots(void *heap, void *context, int phase) {\n    kiln_graph_context *graph = (kiln_graph_context *)context;\n    size_t index;\n    if (phase == KILN_GC_PHASE_RESET) return;\n",
    );
    for (index, (node, _)) in modules.iter().enumerate() {
        let export_count = native_graph_export_count(node);
        if export_count != 0 {
            writeln!(
                output,
                "    for (index = 0; index < (size_t){export_count}; ++index) (void)kiln_realm_gc_mark_value(heap, &kiln_exports_{index}[index]);"
            )
            .unwrap();
        }
        if !node.program.imports.is_empty() {
            writeln!(
                output,
                "    for (index = 0; index < (size_t){}; ++index) (void)kiln_realm_gc_mark_value(heap, &kiln_imports_{index}[index]);",
                node.program.imports.len()
            )
            .unwrap();
        }
    }
    writeln!(
        output,
        "    for (index = 0; index < (size_t){}; ++index) (void)kiln_realm_gc_mark_value(heap, &kiln_namespaces[index]);",
        modules.len()
    )
    .unwrap();
    output.push_str(
        "    if (graph != NULL) {\n        (void)kiln_realm_gc_mark_value(heap, &graph->result);\n        (void)kiln_realm_gc_mark_value(heap, &graph->entry_result);\n    }\n}\n",
    );
}

fn graph_launcher_source(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
) -> Result<String, NativeError> {
    if modules.is_empty() {
        return Err(NativeError::Graph(
            ModuleGraphError::InternalMissingModule {
                path: graph.entry().to_path_buf(),
            },
        ));
    }
    // Diagnostic names remain project-relative, while CommonJS path roles are constructed as
    // distinct types below so observable runtime paths cannot be replaced with diagnostics by a
    // broad path-normalization refactor.
    let logical_paths = modules
        .iter()
        .map(|(node, _)| logical_module_path(graph.project_root(), &node.path))
        .collect::<Result<Vec<_>, _>>()?;
    let mut indices = std::collections::HashMap::with_capacity(modules.len());
    for (index, (node, _)) in modules.iter().enumerate() {
        indices.insert(node.path.clone(), index);
    }
    let namespace_plans = graph_namespace_plans(graph, modules, &indices)?;
    let mut output = String::with_capacity(2048 + modules.len() * 512);
    let mut evaluation = String::with_capacity(modules.len() * 512);
    output.push_str(
        r#"/* Kiln native graph launcher. */
#include <locale.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

enum kiln_value_tag {
    KILN_VALUE_UNDEFINED = 0,
    KILN_VALUE_NULL = 1,
    KILN_VALUE_BOOLEAN = 2,
    KILN_VALUE_NUMBER = 3,
    KILN_VALUE_STRING = 4
};

typedef struct kiln_value {
    uint32_t tag;
    uint32_t len;
    union { uint64_t bits; const char *ptr; };
} kiln_value;

static kiln_value kiln_graph_diagnostic(const char *phase, const char *module) {
    kiln_value value = { KILN_VALUE_STRING, 0, {0} };
    value.ptr = phase;
    static char text[2048];
    size_t phase_len = strlen(phase);
    size_t module_len = strlen(module);
    const char prefix[] = "phase=";
    const char middle[] = " module=";
    size_t total = sizeof(prefix) - 1 + phase_len + sizeof(middle) - 1 + module_len;
    if (total >= sizeof(text)) total = sizeof(text) - 1;
    value.len = 0;
    memcpy(text + value.len, prefix, sizeof(prefix) - 1); value.len += sizeof(prefix) - 1;
    if (value.len + phase_len > total) phase_len = total - value.len;
    memcpy(text + value.len, phase, phase_len); value.len += phase_len;
    if (value.len < total) {
        size_t take = sizeof(middle) - 1;
        if (take > total - value.len) take = total - value.len;
        memcpy(text + value.len, middle, take); value.len += take;
    }
    if (value.len < total) {
        size_t take = module_len;
        if (take > total - value.len) take = total - value.len;
        memcpy(text + value.len, module, take); value.len += take;
    }
    text[value.len] = '\0';
    value.ptr = text;
    return value;
}

typedef int (*kiln_host_call_fn)(void *context, uint16_t operation, uint16_t argc,
                                 const kiln_value *arguments, kiln_value *result);
typedef struct kiln_host {
    kiln_host_call_fn call;
    void *context;
} kiln_host;

typedef int (*kiln_graph_module_fn)(kiln_host *host, kiln_value *result,
                                    const kiln_value *imports, size_t import_count,
                                    kiln_value *exports, size_t export_count);
typedef int (*kiln_commonjs_require_fn)(void *context, kiln_host *host, kiln_value specifier,
                                       kiln_value *result, kiln_value *thrown);
typedef int (*kiln_realm_entry_fn)(void *context);

extern int kiln_cli_host_call(void *context, uint16_t operation, uint16_t argc,
                              const kiln_value *arguments, kiln_value *result);
extern int kiln_cli_write_result(kiln_value value);
extern void kiln_cli_report_failure(int status, kiln_value value);
extern int kiln_realm_create(uint64_t *out_realm);
extern int kiln_realm_enter(uint64_t realm, kiln_realm_entry_fn entry, void *context);
extern int kiln_realm_set_argv(uint64_t realm, uint64_t argc, const char *const *argv);
extern int kiln_realm_drive(uint64_t realm, void *host, void *thrown);
extern int kiln_realm_destroy(uint64_t realm);
typedef void (*kiln_gc_roots_fn)(void *heap, void *context, int phase);
extern int kiln_realm_gc_register_roots(kiln_gc_roots_fn roots, void *context, uint32_t flags);
extern int kiln_realm_gc_unregister_roots(kiln_gc_roots_fn roots, void *context);
extern int kiln_realm_gc_mark_value(void *heap, const void *value);
#define KILN_GC_ROOT_SURVIVES_TEARDOWN 1u
#define KILN_GC_PHASE_RESET 2
"#,
    );
    for (_, module) in modules {
        writeln!(
            output,
            "extern int {}(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count);",
            module.symbol
        )
        .unwrap();
        writeln!(
            output,
            "extern int {}_namespace_wrapper(kiln_value *result, const char *const *names, const size_t *name_lengths, const kiln_value *values, size_t count);",
            module.symbol
        )
        .unwrap();
    }
    for (node, module) in modules {
        if node.source == ModuleSource::CommonJs {
            writeln!(
                output,
                "extern int {}_commonjs_wrapper(kiln_value *imports, size_t import_count, const char *filename, size_t filename_len, const char *dirname, size_t dirname_len, kiln_commonjs_require_fn require_call, void *require_context);",
                module.symbol
            )
            .unwrap();
        }
    }
    for (index, (node, _)) in modules.iter().enumerate() {
        let export_count = native_graph_export_count(node);
        if export_count != 0 {
            writeln!(
                output,
                "static kiln_value kiln_exports_{index}[{}];",
                export_count
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
    writeln!(
        output,
        "static kiln_value kiln_namespaces[{}];",
        modules.len()
    )
    .unwrap();
    output.push_str(
        r#"
typedef struct kiln_commonjs_context {
    kiln_graph_module_fn module;
    kiln_host *host;
    kiln_value *imports;
    size_t import_count;
    kiln_value *exports;
    int state;
} kiln_commonjs_context;
"#,
    );
    for (index, (node, _)) in modules.iter().enumerate() {
        if node.source == ModuleSource::CommonJs {
            writeln!(
                output,
                "static kiln_commonjs_context kiln_commonjs_{index};"
            )
            .unwrap();
        }
    }
    for (index, (node, _)) in modules.iter().enumerate() {
        if node.source == ModuleSource::CommonJs {
            writeln!(
                output,
                "static int kiln_commonjs_require_{index}(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown);"
            )
            .unwrap();
        }
    }
    output.push_str(
        r#"
typedef struct kiln_graph_entry_context {
    kiln_graph_module_fn module;
    kiln_host *host;
    kiln_value *result;
    const kiln_value *imports;
    size_t import_count;
    kiln_value *exports;
    size_t export_count;
} kiln_graph_entry_context;

static int kiln_graph_load_commonjs(kiln_commonjs_context *context, kiln_value *result,
                                    kiln_value *thrown) {
    int status;
    if (context == NULL || result == NULL || context->exports == NULL) return -1;
    if (context->state != 0) {
        *result = context->exports[0];
        return 0;
    }
    context->state = 1;
    status = context->module(context->host, result, context->imports, context->import_count,
                             context->exports, (size_t)1);
    if (status != 0) {
        context->state = 0;
        if (thrown != NULL) *thrown = *result;
        return status;
    }
    context->state = 2;
    *result = context->exports[0];
    return 0;
}

"#,
    );
    for (index, (node, _)) in modules.iter().enumerate() {
        if node.source != ModuleSource::CommonJs {
            continue;
        }
        writeln!(
            output,
            "\nstatic int kiln_commonjs_require_{index}(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {{\n    (void)opaque;\n    (void)host;\n    if (result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return -1;"
        )
        .unwrap();
        for dependency in &node.dependencies {
            let provider_index = *indices.get(&dependency.target).ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::InternalMissingModule {
                    path: dependency.target.clone(),
                })
            })?;
            let specifier = native_c_string(&dependency.specifier);
            let specifier_len = dependency.specifier.len();
            if modules[provider_index].0.source == ModuleSource::CommonJs {
                writeln!(
                    output,
                    "    if (specifier.len == (size_t){specifier_len} && memcmp(specifier.ptr, {specifier}, (size_t){specifier_len}) == 0) return kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, thrown);"
                )
                .unwrap();
            } else {
                let commonjs_export = native_commonjs_projection_index(modules[provider_index].0)
                    .ok_or_else(|| {
                    NativeError::Graph(ModuleGraphError::MissingExport {
                        importer: node.path.clone(),
                        provider: dependency.target.clone(),
                        name: "CommonJS module.exports projection".to_owned(),
                    })
                })?;
                writeln!(
                    output,
                    "    if (specifier.len == (size_t){specifier_len} && memcmp(specifier.ptr, {specifier}, (size_t){specifier_len}) == 0) {{ *result = kiln_exports_{provider_index}[{commonjs_export}]; return 0; }}"
                )
                .unwrap();
            }
        }
        output.push_str("    if (thrown != NULL) *thrown = specifier;\n    return -2;\n}\n");
    }
    output.push_str(
        r#"
typedef struct kiln_graph_context {
    kiln_host host;
    kiln_value result;
    kiln_value entry_result;
} kiln_graph_context;
"#,
    );
    render_graph_roots(&mut output, modules);
    output.push_str(
        r#"
static int kiln_graph_enter(void *opaque) {
    kiln_graph_context *graph = (kiln_graph_context *)opaque;
    kiln_graph_entry_context context;
    kiln_host *host;
    kiln_value *result;
    int status = 0;
    if (graph == NULL) return -1;
    status = kiln_realm_gc_register_roots(kiln_graph_gc_roots, graph, KILN_GC_ROOT_SURVIVES_TEARDOWN);
    if (status != 0) return status;
    host = &graph->host;
    result = &graph->result;
"#,
    );
    // CommonJS cache identities and wrapper locals exist before any module body runs, so cycles
    // can return the initially allocated exports object without re-entering a module.
    for (index, (node, module)) in modules.iter().enumerate() {
        if node.source == ModuleSource::CommonJs {
            // CommonJS observes the canonical filesystem identity through `__filename` and
            // `__dirname`. Unlike the diagnostic names below, these values are language
            // semantics: making them project-relative would disagree with Node and the
            // interpreter. A CommonJS launcher is therefore intentionally checkout-specific
            // while its module objects remain reusable across roots.
            let paths = CommonJsModulePaths::new(graph.project_root(), &node.path)?;
            write_graph_commonjs_realm_setup(
                &mut evaluation,
                index,
                &module.symbol,
                &paths.filename,
                &paths.dirname,
                &paths.diagnostic,
            );
        }
    }
    for (index, (node, module)) in modules.iter().enumerate() {
        for (import_index, import) in node.program.imports.iter().enumerate() {
            match graph_import_value(graph, modules, &indices, node, import)? {
                NativeGraphImport::Internal => {}
                NativeGraphImport::Binding {
                    provider_index,
                    export_index,
                } => {
                    if modules[provider_index].0.source == ModuleSource::CommonJs {
                        writeln!(
                            evaluation,
                            "    status = kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, result);\n    if (status != 0) return status;"
                        )
                        .unwrap();
                    }
                    writeln!(
                        evaluation,
                        "    kiln_imports_{index}[{import_index}] = kiln_exports_{provider_index}[{export_index}];"
                    )
                    .unwrap();
                }
                NativeGraphImport::Namespace { provider_index } => {
                    writeln!(
                        evaluation,
                        "    kiln_imports_{index}[{import_index}] = kiln_namespaces[{provider_index}];"
                    )
                    .unwrap();
                }
            }
        }
        let imports = if node.program.imports.is_empty() {
            "NULL".to_owned()
        } else {
            format!("kiln_imports_{index}")
        };
        let export_count = native_graph_export_count(node);
        let exports = if export_count == 0 {
            "NULL".to_owned()
        } else {
            format!("kiln_exports_{index}")
        };
        if node.source == ModuleSource::CommonJs {
            writeln!(
                evaluation,
                "    status = kiln_graph_load_commonjs(&kiln_commonjs_{index}, result, result);\n    if (status != 0) {{ if (result->tag != KILN_VALUE_STRING || result->len == 0) *result = kiln_graph_diagnostic(\"commonjs-evaluate\", {}); return status; }}",
                native_c_string(&logical_paths[index])
            )
            .unwrap();
            if node.path == graph.entry() {
                evaluation.push_str("    graph->entry_result = *result;\n");
            }
            if let Some(plan) = &namespace_plans[index] {
                write_graph_namespace_build(&mut evaluation, index, &module.symbol, plan, false);
                writeln!(
                    evaluation,
                    "    if (status != 0) {{ *result = kiln_graph_diagnostic(\"namespace\", {}); return status; }}",
                    native_c_string(&logical_paths[index])
                )
                .unwrap();
            }
            continue;
        }
        writeln!(evaluation, "    context.module = {};", module.symbol).unwrap();
        evaluation.push_str("    context.host = host;\n    context.result = result;\n");
        writeln!(evaluation, "    context.imports = {imports};").unwrap();
        writeln!(
            evaluation,
            "    context.import_count = (size_t){};",
            node.program.imports.len()
        )
        .unwrap();
        writeln!(evaluation, "    context.exports = {exports};").unwrap();
        writeln!(
            evaluation,
            "    context.export_count = (size_t){};",
            export_count
        )
        .unwrap();
        evaluation.push_str(
            "    status = context.module(context.host, context.result, context.imports, context.import_count, context.exports, context.export_count);\n",
        );
        // A unit that stops says why. An uncaught throw arrives as text in `result`, because a
        // reference cannot cross the boundary and printing it is the only thing left to do with
        // it -- exiting 1 in silence is the failure this corpus exists to make impossible.
        //
        // The name it says is the project-relative one, not `node.path`. An absolute path is a
        // property of the machine that ran the build rather than of the program: it makes the
        // launcher source differ between two checkouts of the same graph, which costs every
        // sibling root the launcher object it should have reused, and it puts the builder's home
        // directory inside the shipped executable.
        writeln!(
            evaluation,
            "    if (status != 0) {{ if (result->tag != KILN_VALUE_STRING || result->len == 0) *result = kiln_graph_diagnostic(\"module-enter\", {}); return status; }}",
            native_c_string(&logical_paths[index])
        )
        .unwrap();
        if node.path == graph.entry() {
            evaluation.push_str("    graph->entry_result = *result;\n");
        }
        if let Some(plan) = &namespace_plans[index] {
            write_graph_namespace_build(&mut evaluation, index, &module.symbol, plan, false);
            writeln!(
                evaluation,
                "    if (status != 0) {{ *result = kiln_graph_diagnostic(\"namespace\", {}); return status; }}",
                native_c_string(&logical_paths[index])
            )
            .unwrap();
        }
    }
    output.push_str(&evaluation);
    output.push_str(
        "    return 0;\n}\n\n/* argc/argv reach the realm before it is entered, so the first `process` a module builds\n * already has the command line. The strings are copied on the other side of this call. */\nint main(int argc, char **argv) {\n    kiln_graph_context graph = {0};\n    uint64_t realm = 0;\n    int status;\n    int destroy_status;\n    graph.result.tag = KILN_VALUE_UNDEFINED;\n    graph.entry_result.tag = KILN_VALUE_UNDEFINED;\n    if (setlocale(LC_NUMERIC, \"C\") == NULL) return 1;\n    graph.host.call = kiln_cli_host_call;\n    graph.host.context = NULL;\n    status = kiln_realm_create(&realm);\n    if (status == 0 && argc > 0)\n        status = kiln_realm_set_argv(realm, (uint64_t)argc, (const char *const *)argv);\n    if (status == 0) status = kiln_realm_enter(realm, kiln_graph_enter, &graph);\n    if (status == 0) status = kiln_realm_drive(realm, &graph.host, &graph.result);\n    if (status != 0) kiln_cli_report_failure(status, graph.result);\n    if (status == 0 && kiln_cli_write_result(graph.entry_result) != 0) status = 1;\n    destroy_status = kiln_realm_destroy(realm);\n    if (status == 0) status = destroy_status;\n    return status == 0 ? 0 : 1;\n}\n",
    );
    Ok(output)
}

fn graph_shared_header(wrapper_symbol: &str) -> Result<String, NativeError> {
    // The graph wrapper has the same public two-argument signature as a scalar embedded module,
    // so reuse the runtime's canonical symbol validation and include-guarded header generator.
    embedding_module_header(wrapper_symbol).map_err(NativeError::from)
}

/// Append the v2 owned-result ABI adapter to a shared-library translation unit.
///
/// The adapter deliberately calls the already-generated v1 entry point.  That keeps the two
/// entry points semantically identical while making ownership explicit at the DSO boundary.
/// `prefix` is only used to make the private helper names easy to identify in generated C; the
/// public accessor always follows the ABI-mandated `{base}_api_v2` spelling.
fn owned_result_wrapper_source(
    base: &str,
    prefix: &str,
    invocation: &str,
    post_invoke_cleanup: &str,
) -> String {
    let stem = format!("{}_{}", prefix, base);
    let arena = format!("kiln_v2_{stem}_arena");
    let proxy = format!("kiln_v2_{stem}_proxy");
    let run = format!("kiln_v2_{stem}_run");
    let read = format!("kiln_v2_{stem}_read");
    let dispose = format!("kiln_v2_{stem}_dispose");
    let descriptor = format!("kiln_v2_{stem}_descriptor");
    let api_symbol = format!("{base}_api_v2");
    format!(
        r#"
/* Kiln native owned-result API v2 adapter for {base}. */
#ifndef KILN_MODULE_API_VERSION
#define KILN_MODULE_API_VERSION {api_version}
typedef struct kiln_result_v2 kiln_result_v2;
typedef struct kiln_module_api_v2 {{
    uint32_t abi_version;
    uint32_t struct_size;
    int (*run)(kiln_host *host, kiln_result_v2 **result);
    int (*read)(const kiln_result_v2 *result, kiln_value *value);
    void (*dispose)(kiln_result_v2 **result);
}} kiln_module_api_v2;
#endif

struct kiln_result_v2 {{
    kiln_value value;
}};

typedef struct {arena} {{
    void **allocations;
    size_t count;
    size_t capacity;
}} {arena};

typedef struct {proxy} {{
    kiln_host *upstream;
    {arena} *arena;
    int failure_status;
}} {proxy};

static void {arena}_clear({arena} *arena) {{
    size_t index;
    if (arena == NULL) return;
    for (index = 0; index < arena->count; ++index) free(arena->allocations[index]);
    free(arena->allocations);
    arena->allocations = NULL;
    arena->count = 0;
    arena->capacity = 0;
}}

static int {arena}_track({arena} *arena, void *allocation) {{
    size_t capacity;
    void **grown;
    if (arena == NULL || allocation == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (arena->count == arena->capacity) {{
        capacity = arena->capacity == 0 ? 8 : arena->capacity;
        if (capacity > (size_t)-1 / 2) return KILN_C99_HOST_ERROR;
        capacity *= 2;
        if (capacity > (size_t)-1 / sizeof(*grown)) return KILN_C99_HOST_ERROR;
        grown = (void **)realloc(arena->allocations, capacity * sizeof(*grown));
        if (grown == NULL) return KILN_C99_HOST_ERROR;
        arena->allocations = grown;
        arena->capacity = capacity;
    }}
    arena->allocations[arena->count++] = allocation;
    return KILN_C99_OK;
}}

static int {stem}_proxy_call(void *context, uint16_t operation, uint16_t argc,
                             const kiln_value *arguments, kiln_value *result) {{
    {proxy} *proxy = ({proxy} *)context;
    kiln_host *upstream;
    char *copy;
    int status;
    if (proxy == NULL || proxy->upstream == NULL || proxy->upstream->call == NULL ||
        result == NULL) return KILN_C99_HOST_ERROR;
    upstream = proxy->upstream;
    status = upstream->call(upstream->context, operation, argc, arguments, result);
    if (status != KILN_C99_OK) return status;
    if (result->tag > KILN_VALUE_STRING) {{
        proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
        return KILN_C99_INVALID_ARGUMENT;
    }}
    if (result->tag != KILN_VALUE_STRING) return KILN_C99_OK;
    if (result->len == (size_t)-1 || (result->len != 0 && result->ptr == NULL)) {{
        proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
        return KILN_C99_INVALID_ARGUMENT;
    }}
    copy = (char *)malloc(result->len == 0 ? 1 : result->len);
    if (copy == NULL) {{
        proxy->failure_status = KILN_C99_HOST_ERROR;
        return KILN_C99_HOST_ERROR;
    }}
    if (result->len != 0) memcpy(copy, result->ptr, result->len);
    status = {arena}_track(proxy->arena, copy);
    if (status != KILN_C99_OK) {{
        free(copy);
        proxy->failure_status = KILN_C99_HOST_ERROR;
        return KILN_C99_HOST_ERROR;
    }}
    result->ptr = copy;
    return KILN_C99_OK;
}}

static int {run}(kiln_host *host, kiln_result_v2 **out) {{
    {arena} arena = {{ NULL, 0, 0 }};
    {proxy} proxy = {{ host, &arena, KILN_C99_OK }};
    kiln_host proxy_host = {{ {stem}_proxy_call, &proxy }};
    kiln_value value = {{ KILN_VALUE_UNDEFINED, 0, {{0}} }};
    kiln_result_v2 *owned = NULL;
    char *copy = NULL;
    int status;
    if (out == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out = NULL;
    status = {invocation};
    if (proxy.failure_status != KILN_C99_OK) status = proxy.failure_status;
    if (status != KILN_C99_OK && status != KILN_C99_THROWN) goto cleanup;
    if (value.tag > KILN_VALUE_STRING) {{
        status = KILN_C99_INVALID_ARGUMENT;
        goto cleanup;
    }}
    if (value.tag == KILN_VALUE_STRING &&
        (value.len == (size_t)-1 || (value.len != 0 && value.ptr == NULL))) {{
        status = KILN_C99_INVALID_ARGUMENT;
        goto cleanup;
    }}
    owned = (kiln_result_v2 *)malloc(sizeof(*owned));
    if (owned == NULL) {{
        status = KILN_C99_HOST_ERROR;
        goto cleanup;
    }}
    owned->value = value;
    if (value.tag == KILN_VALUE_STRING) {{
        copy = (char *)malloc(value.len == 0 ? 1 : value.len);
        if (copy == NULL) {{
            status = KILN_C99_HOST_ERROR;
            goto cleanup;
        }}
        if (value.len != 0) memcpy(copy, value.ptr, value.len);
        owned->value.ptr = copy;
        copy = NULL;
    }} else {{
        /* `ptr` shares the payload word with `bits`, so clearing it here would erase the number
         * or boolean this value carries. A non-string has no pointer that could dangle; only the
         * length needs normalising. */
        owned->value.len = 0;
    }}
    *out = owned;
    owned = NULL;
cleanup:
    {post_invoke_cleanup}
    free(copy);
    free(owned);
    {arena}_clear(&arena);
    return status;
}}

static int {read}(const kiln_result_v2 *result, kiln_value *value) {{
    if (result == NULL || value == NULL) return KILN_C99_INVALID_ARGUMENT;
    *value = result->value;
    return KILN_C99_OK;
}}

static void {dispose}(kiln_result_v2 **result) {{
    if (result == NULL || *result == NULL) return;
    /* Only a string owns heap bytes. `ptr` shares the payload word with `bits`, so freeing it
     * unconditionally would pass a number's bit pattern to `free`, and clearing it afterwards
     * would erase the value. */
    if ((*result)->value.tag == KILN_VALUE_STRING) {{
        free((void *)(*result)->value.ptr);
        (*result)->value.ptr = NULL;
    }}
    (*result)->value.len = 0;
    free(*result);
    *result = NULL;
}}

static const kiln_module_api_v2 {descriptor} = {{
    KILN_MODULE_API_VERSION,
    (uint32_t)sizeof(kiln_module_api_v2),
    {run},
    {read},
    {dispose}
}};

const kiln_module_api_v2 *{api_symbol}(void) {{
    return &{descriptor};
}}
"#,
        api_version = EMBEDDING_MODULE_API_VERSION,
        api_symbol = api_symbol,
    )
}

/// The embedding wrapper's root function (ADR 0041): the link arrays its body allocates, for as
/// long as they exist, and the module result slot the body writes into.
fn render_shared_wrapper_roots(
    output: &mut String,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    wrapper_symbol: &str,
) {
    writeln!(
        output,
        "\ntypedef struct {wrapper_symbol}_gc_context {{\n    kiln_value **imports;\n    kiln_value **exports;\n    kiln_value *namespaces;\n    kiln_value *result;\n}} {wrapper_symbol}_gc_context;"
    )
    .unwrap();
    writeln!(
        output,
        "static const size_t {wrapper_symbol}_gc_import_counts[{}] = {{{}}};",
        modules.len().max(1),
        modules
            .iter()
            .map(|(node, _)| node.program.imports.len().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
    .unwrap();
    writeln!(
        output,
        "static const size_t {wrapper_symbol}_gc_export_counts[{}] = {{{}}};",
        modules.len().max(1),
        modules
            .iter()
            .map(|(node, _)| native_graph_export_count(node).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
    .unwrap();
    writeln!(
        output,
        "static void {wrapper_symbol}_gc_roots(void *heap, void *context, int phase) {{\n    {wrapper_symbol}_gc_context *roots = ({wrapper_symbol}_gc_context *)context;\n    size_t module;\n    size_t index;\n    if (phase == KILN_GC_PHASE_RESET || roots == NULL) return;\n    for (module = 0; module < (size_t){count}; ++module) {{\n        if (roots->imports != NULL && roots->imports[module] != NULL)\n            for (index = 0; index < {wrapper_symbol}_gc_import_counts[module]; ++index)\n                (void)kiln_realm_gc_mark_value(heap, &roots->imports[module][index]);\n        if (roots->exports != NULL && roots->exports[module] != NULL)\n            for (index = 0; index < {wrapper_symbol}_gc_export_counts[module]; ++index)\n                (void)kiln_realm_gc_mark_value(heap, &roots->exports[module][index]);\n        if (roots->namespaces != NULL) (void)kiln_realm_gc_mark_value(heap, &roots->namespaces[module]);\n    }}\n    if (roots->result != NULL) (void)kiln_realm_gc_mark_value(heap, roots->result);\n}}",
        count = modules.len()
    )
    .unwrap();
}

fn graph_shared_wrapper_source(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    wrapper_symbol: &str,
) -> Result<String, NativeError> {
    if modules.is_empty() {
        return Err(NativeError::Graph(
            ModuleGraphError::InternalMissingModule {
                path: graph.entry().to_path_buf(),
            },
        ));
    }
    let impl_symbol = format!("{}_impl", wrapper_symbol);
    let mut indices = std::collections::HashMap::with_capacity(modules.len());
    for (index, (node, _)) in modules.iter().enumerate() {
        indices.insert(node.path.clone(), index);
    }
    let namespace_plans = graph_namespace_plans(graph, modules, &indices)?;
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
    output.push_str(
        r#"typedef int (*kiln_realm_entry_fn)(void *context);
extern int kiln_realm_create(uint64_t *out_realm);
extern int kiln_realm_enter(uint64_t realm, kiln_realm_entry_fn entry, void *context);
extern int kiln_realm_drive(uint64_t realm, void *host, void *thrown);
extern int kiln_realm_destroy(uint64_t realm);
typedef void (*kiln_gc_roots_fn)(void *heap, void *context, int phase);
extern int kiln_realm_gc_register_roots(kiln_gc_roots_fn roots, void *context, uint32_t flags);
extern int kiln_realm_gc_unregister_roots(kiln_gc_roots_fn roots, void *context);
extern int kiln_realm_gc_mark_value(void *heap, const void *value);
#define KILN_GC_ROOT_SURVIVES_TEARDOWN 1u
#define KILN_GC_PHASE_RESET 2
extern int kiln_realm_is_active(void);

typedef int (*kiln_graph_module_fn)(kiln_host *host, kiln_value *result,
                                    const kiln_value *imports, size_t import_count,
                                    kiln_value *exports, size_t export_count);
typedef int (*kiln_commonjs_require_fn)(void *context, kiln_host *host, kiln_value specifier,
                                        kiln_value *result, kiln_value *thrown);

"#,
    );
    for (_, module) in modules {
        writeln!(
            output,
            "extern int {}(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count);",
            module.symbol
        )
        .unwrap();
        writeln!(
            output,
            "extern int {}_namespace_wrapper(kiln_value *result, const char *const *names, const size_t *name_lengths, const kiln_value *values, size_t count);",
            module.symbol
        )
        .unwrap();
    }
    output.push_str(
        r#"
typedef struct kiln_commonjs_context {
    kiln_graph_module_fn module;
    kiln_host *host;
    kiln_value *imports;
    size_t import_count;
    kiln_value *exports;
    int state;
} kiln_commonjs_context;

typedef struct kiln_commonjs_graph_context {
    kiln_commonjs_context *modules;
    kiln_value **exports;
    size_t module_count;
} kiln_commonjs_graph_context;

static int kiln_graph_load_commonjs(kiln_commonjs_context *context, kiln_value *result,
                                    kiln_value *thrown) {
    int status;
    if (context == NULL || result == NULL || context->exports == NULL) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    if (context->state != 0) {
        *result = context->exports[0];
        return KILN_C99_OK;
    }
    context->state = 1;
    status = context->module(context->host, result, context->imports, context->import_count,
                             context->exports, (size_t)1);
    if (status != KILN_C99_OK) {
        context->state = 0;
        if (thrown != NULL) *thrown = *result;
        return status;
    }
    context->state = 2;
    *result = context->exports[0];
    return KILN_C99_OK;
}
"#,
    );
    for (node, module) in modules {
        if node.source == ModuleSource::CommonJs {
            writeln!(
                output,
                "extern int {}_commonjs_wrapper(kiln_value *imports, size_t import_count, const char *filename, size_t filename_len, const char *dirname, size_t dirname_len, kiln_commonjs_require_fn require_call, void *require_context);",
                module.symbol
            )
            .unwrap();
        }
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
    int failure_status;
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
    if (result->len == (size_t)-1 || (result->len != 0 && result->ptr == NULL)) {
        proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
        return KILN_C99_INVALID_ARGUMENT;
    }
    copy = (char *)malloc(result->len == 0 ? 1 : result->len);
    if (copy == NULL) {
        proxy->failure_status = KILN_C99_HOST_ERROR;
        return KILN_C99_HOST_ERROR;
    }
    if (result->len != 0) memcpy(copy, result->ptr, result->len);
    status = kiln_proxy_arena_track(arena, copy);
    if (status != KILN_C99_OK) {
        free(copy);
        proxy->failure_status = KILN_C99_HOST_ERROR;
        return KILN_C99_HOST_ERROR;
    }
    result->ptr = copy;
    return KILN_C99_OK;
}

"#,
    );
    for (index, (node, _)) in modules.iter().enumerate() {
        if node.source != ModuleSource::CommonJs {
            continue;
        }
        writeln!(
            output,
            "\nstatic int kiln_commonjs_require_{index}(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {{\n    kiln_commonjs_graph_context *graph = (kiln_commonjs_graph_context *)opaque;\n    (void)host;\n    if (graph == NULL || result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;"
        )
        .unwrap();
        for dependency in &node.dependencies {
            let provider_index = *indices.get(&dependency.target).ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::InternalMissingModule {
                    path: dependency.target.clone(),
                })
            })?;
            let specifier = native_c_string(&dependency.specifier);
            let specifier_len = dependency.specifier.len();
            if modules[provider_index].0.source == ModuleSource::CommonJs {
                writeln!(
                    output,
                    "    if (specifier.len == (size_t){specifier_len} && memcmp(specifier.ptr, {specifier}, (size_t){specifier_len}) == 0) return kiln_graph_load_commonjs(&graph->modules[{provider_index}], result, thrown);"
                )
                .unwrap();
            } else {
                let commonjs_export = native_commonjs_projection_index(modules[provider_index].0)
                    .ok_or_else(|| {
                    NativeError::Graph(ModuleGraphError::MissingExport {
                        importer: node.path.clone(),
                        provider: dependency.target.clone(),
                        name: "CommonJS module.exports projection".to_owned(),
                    })
                })?;
                writeln!(
                    output,
                    "    if (specifier.len == (size_t){specifier_len} && memcmp(specifier.ptr, {specifier}, (size_t){specifier_len}) == 0) {{ *result = graph->exports[{provider_index}][{commonjs_export}]; return KILN_C99_OK; }}"
                )
                .unwrap();
            }
        }
        output.push_str(
            "    if (thrown != NULL) *thrown = specifier;\n    return KILN_C99_HOST_ERROR;\n}\n",
        );
    }
    render_shared_wrapper_roots(&mut output, modules, wrapper_symbol);
    writeln!(
        output,
        "\nstatic int {wrapper_symbol}_body(kiln_host *proxy_host, kiln_value *result) {{\n    kiln_value **import_arrays = NULL;\n    kiln_value **export_arrays = NULL;\n    kiln_value *namespaces = NULL;\n    kiln_commonjs_context *commonjs_modules = NULL;\n    kiln_commonjs_graph_context commonjs_graph;\n    {wrapper_symbol}_gc_context gc_context;\n    kiln_value module_result;\n    int status = KILN_C99_OK;\n    size_t index;\n    if (result == NULL || proxy_host == NULL) return KILN_C99_INVALID_ARGUMENT;\n    commonjs_graph = (kiln_commonjs_graph_context){{ NULL, NULL, (size_t){count} }};\n    *result = (kiln_value){{ KILN_VALUE_UNDEFINED, 0, {{0}} }};\n    if ((size_t){count} > (size_t)-1 / sizeof(*import_arrays) ||\n        (size_t){count} > (size_t)-1 / sizeof(*export_arrays) ||\n        (size_t){count} > (size_t)-1 / sizeof(*namespaces) ||\n        (size_t){count} > (size_t)-1 / sizeof(*commonjs_modules)) {{\n        status = KILN_C99_HOST_ERROR;\n        goto cleanup;\n    }}\n    import_arrays = (kiln_value **)calloc((size_t){count}, sizeof(*import_arrays));\n    export_arrays = (kiln_value **)calloc((size_t){count}, sizeof(*export_arrays));\n    namespaces = (kiln_value *)calloc((size_t){count}, sizeof(*namespaces));\n    commonjs_modules = (kiln_commonjs_context *)calloc((size_t){count}, sizeof(*commonjs_modules));\n    if (import_arrays == NULL || export_arrays == NULL || namespaces == NULL || commonjs_modules == NULL) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}\n    commonjs_graph.modules = commonjs_modules;\n    commonjs_graph.exports = export_arrays;\n    gc_context.imports = import_arrays;\n    gc_context.exports = export_arrays;\n    gc_context.namespaces = namespaces;\n    gc_context.result = &module_result;\n    module_result = (kiln_value){{ KILN_VALUE_UNDEFINED, 0, {{0}} }};\n    status = kiln_realm_gc_register_roots({wrapper_symbol}_gc_roots, &gc_context, 0);\n    if (status != KILN_C99_OK) goto cleanup;",
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
        let export_count = native_graph_export_count(node);
        if export_count != 0 {
            writeln!(
                output,
                "    if ((size_t){export_count} > (size_t)-1 / sizeof(**export_arrays)) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}\n    export_arrays[{index}] = (kiln_value *)malloc((size_t){export_count} * sizeof(**export_arrays));\n    if (export_arrays[{index}] == NULL) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}",
            )
            .unwrap();
        }
    }
    for (index, (node, module)) in modules.iter().enumerate() {
        if node.source != ModuleSource::CommonJs {
            continue;
        }
        let paths = CommonJsModulePaths::new(graph.project_root(), &node.path)?;
        write_graph_commonjs_shared_setup(
            &mut output,
            index,
            &module.symbol,
            &paths.filename,
            &paths.dirname,
        );
    }
    for (index, (node, _)) in modules.iter().enumerate() {
        for (import_index, import) in node.program.imports.iter().enumerate() {
            match graph_import_value(graph, modules, &indices, node, import)? {
                NativeGraphImport::Internal => {}
                NativeGraphImport::Binding {
                    provider_index,
                    export_index,
                } => {
                    writeln!(
                        output,
                        "    import_arrays[{index}][{import_index}] = export_arrays[{provider_index}][{export_index}];"
                    )
                    .unwrap();
                }
                NativeGraphImport::Namespace { provider_index } => {
                    writeln!(
                        output,
                        "    import_arrays[{index}][{import_index}] = namespaces[{provider_index}];"
                    )
                    .unwrap();
                }
            }
        }
        let import_count = node.program.imports.len();
        let export_count = native_graph_export_count(node);
        if node.source == ModuleSource::CommonJs {
            writeln!(
                output,
                "    status = kiln_graph_load_commonjs(&commonjs_modules[{index}], &module_result, &module_result);\n    if (status != KILN_C99_OK) {{ *result = module_result; goto cleanup; }}"
            )
            .unwrap();
            if index == entry_index {
                output.push_str("    *result = module_result;\n");
            }
            if let Some(plan) = &namespace_plans[index] {
                write_graph_namespace_build(
                    &mut output,
                    index,
                    &modules[index].1.symbol,
                    plan,
                    true,
                );
                output.push_str("    if (status != KILN_C99_OK) goto cleanup;\n");
            }
            continue;
        }
        writeln!(
            output,
            "    module_result = (kiln_value){{ KILN_VALUE_UNDEFINED, 0, {{0}} }};\n    status = {}(proxy_host, &module_result, import_arrays[{index}], (size_t){import_count}, export_arrays[{index}], (size_t){export_count});\n    if (status != KILN_C99_OK) {{ *result = module_result; goto cleanup; }}",
            modules[index].1.symbol
        )
        .unwrap();
        if index == entry_index {
            output.push_str("    *result = module_result;\n");
        }
        if let Some(plan) = &namespace_plans[index] {
            write_graph_namespace_build(&mut output, index, &modules[index].1.symbol, plan, true);
            output.push_str("    if (status != KILN_C99_OK) goto cleanup;\n");
        }
    }
    output.push_str("cleanup:\n");
    writeln!(
        output,
        "    (void)kiln_realm_gc_unregister_roots({wrapper_symbol}_gc_roots, &gc_context);"
    )
    .unwrap();
    output.push_str("    for (index = 0; index < (size_t)");
    write!(output, "{}; ++index) {{\n        free(import_arrays == NULL ? NULL : import_arrays[index]);\n        free(export_arrays == NULL ? NULL : export_arrays[index]);\n    }}\n    free(import_arrays);\n    free(export_arrays);\n    free(namespaces);\n    free(commonjs_modules);\n    return status;\n}}\n", modules.len()).unwrap();
    writeln!(
        output,
        r#"
typedef struct {wrapper_symbol}_realm_context {{
    kiln_value *result;
    kiln_proxy_arena arena;
    kiln_proxy_host proxy;
    kiln_host proxy_host;
}} {wrapper_symbol}_realm_context;

static void {wrapper_symbol}_gc_result_roots(void *heap, void *context, int phase) {{
    if (phase == KILN_GC_PHASE_RESET || context == NULL) return;
    (void)kiln_realm_gc_mark_value(heap, context);
}}

static int {wrapper_symbol}_realm_entry(void *opaque) {{
    {wrapper_symbol}_realm_context *context = ({wrapper_symbol}_realm_context *)opaque;
    int status;
    if (context == NULL) return KILN_C99_INVALID_ARGUMENT;
    status = kiln_realm_gc_register_roots({wrapper_symbol}_gc_result_roots, context->result,
                                          KILN_GC_ROOT_SURVIVES_TEARDOWN);
    if (status != KILN_C99_OK) return status;
    return {wrapper_symbol}_body(&context->proxy_host, context->result);
}}

static int {impl_symbol}(kiln_host *host, kiln_value *result, int preserve_result) {{
    {wrapper_symbol}_realm_context context;
    uint64_t realm = 0;
    char *preserved = NULL;
    int status;
    int destroy_status;
    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;
    context.result = result;
    context.arena = (kiln_proxy_arena){{ NULL, 0, 0 }};
    context.proxy = (kiln_proxy_host){{ host, &context.arena, KILN_C99_OK }};
    context.proxy_host = (kiln_host){{ kiln_proxy_call, &context.proxy }};
    status = kiln_realm_create(&realm);
    if (status == KILN_C99_OK)
        status = kiln_realm_enter(realm, {wrapper_symbol}_realm_entry, &context);
    if (status == KILN_C99_OK)
        status = kiln_realm_drive(realm, &context.proxy_host, result);
    if (realm != 0) {{
        destroy_status = kiln_realm_destroy(realm);
        if (status == KILN_C99_OK) status = destroy_status;
    }}
    /* Public v1 results remain borrowed; private v2 calls preserve strings across cleanup. */
    if (preserve_result != 0 && result->tag == KILN_VALUE_STRING) {{
        if (result->len == (size_t)-1 || (result->len != 0 && result->ptr == NULL)) {{
            *result = (kiln_value){{ KILN_VALUE_UNDEFINED, 0, {{0}} }};
            status = KILN_C99_INVALID_ARGUMENT;
        }} else {{
            preserved = (char *)malloc(result->len == 0 ? 1 : result->len);
            if (preserved == NULL) {{
                *result = (kiln_value){{ KILN_VALUE_UNDEFINED, 0, {{0}} }};
                status = KILN_C99_HOST_ERROR;
            }} else {{
                if (result->len != 0) memcpy(preserved, result->ptr, result->len);
            }}
        }}
    }}
    kiln_proxy_arena_clear(&context.arena);
    if (preserved != NULL) result->ptr = preserved;
    return status;
}}

int {wrapper_symbol}(kiln_host *host, kiln_value *result) {{
    return {impl_symbol}(host, result, 0);
}}
"#
    )
    .unwrap();
    let invocation = format!("{impl_symbol}(&proxy_host, &value, 1)");
    output.push_str(&owned_result_wrapper_source(
        wrapper_symbol,
        "graph",
        &invocation,
        "if (value.tag == KILN_VALUE_STRING) { free((void *)value.ptr); value.ptr = NULL; value.len = 0; }",
    ));
    Ok(output)
}

fn run_worker<T>(
    stage: &'static str,
    work: impl FnOnce() -> Result<T, NativeError>,
) -> Result<T, NativeError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .map_err(|_| NativeError::WorkerPanic { stage })?
}

/// Runs `work` over `0..count` on Rayon's work-stealing pool.
///
/// Object compile times are wildly uneven: a twenty-line module and a bundled application are both
/// one index here, and they differ by orders of magnitude. Rayon's indexed parallel iterator keeps
/// idle workers useful without making Kiln maintain its own cursor, worker lifecycle or stealing
/// policy.
///
/// Results come back in index order regardless of which worker produced them, and the failure
/// reported is the one belonging to the lowest failing index — either the error `work` returned or
/// [`NativeError::WorkerPanic`] where the task holding that index panicked.
fn compile_indexed<T, F>(count: usize, stage: &'static str, work: F) -> Result<Vec<T>, NativeError>
where
    F: Fn(usize) -> Result<T, NativeError> + Send + Sync,
    T: Send,
{
    (0..count)
        .into_par_iter()
        .map(|index| run_worker(stage, || work(index)))
        .collect::<Vec<_>>()
        .into_iter()
        .collect()
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

#[cfg(test)]
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
    let _process_lock = SHARED_PUBLICATION_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _directory_lock = acquire_shared_publication_lock(output)?;
    materialize_immutable(loader, library)?;
    // Keep the old helper's compatibility-alias ordering for focused materialization tests. The
    // actual shared builders use the transactional package publisher below.
    materialize(&shared_library_header_path(output), header_source, false)?;
    materialize(output, library, false)
}

fn publish_shared_artifacts_with_symbols(
    output: &Path,
    loader: &Path,
    header_source: &[u8],
    library: &[u8],
    legacy_entry_symbol: &str,
    v2_api_symbol: &str,
) -> Result<shared_publication::SharedLibraryPackage, NativeError> {
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
    shared_publication::publish_locked(
        output,
        loader,
        header_source,
        library,
        legacy_entry_symbol,
        v2_api_symbol,
    )
    .map_err(NativeError::from)
}

#[cfg(test)]
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

#[cfg(test)]
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

/// Set to keep a native build's scratch directory instead of deleting it.
///
/// The generated C is the only artifact that can explain a native performance result, and until
/// now nothing could read it: it is written into a temporary directory this `Drop` removes, and
/// the copy in the artifact cache is stored compressed. Measuring the emitted shape therefore
/// meant reconstructing it by hand from `kiln-backend-c`'s snapshots — which is how ADR 0016's
/// own table was produced, and why that table describes an emission *shape* rather than the
/// pipeline that ships. Keeping the directory is the difference between the two.
///
/// An environment variable rather than a flag because this crate is below the CLI and the
/// protocol, and a diagnostic that has to be plumbed through both is a diagnostic nobody reaches
/// for. The path is announced on stderr because a directory kept silently is a directory left
/// behind: this one is not cleaned up, by design, and the operator has to know where it went.
const KEEP_WORK_ENV: &str = "KILN_NATIVE_KEEP_WORK";

impl Drop for WorkDirectory {
    fn drop(&mut self) {
        if std::env::var_os(KEEP_WORK_ENV).is_some() {
            eprintln!(
                "kiln: kept native build directory {} ({KEEP_WORK_ENV} is set)",
                self.path.display()
            );
            return;
        }
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
    public_symbols: &[&str],
    inputs: impl IntoIterator<Item = Digest>,
) -> Digest {
    let identity_options = Digest::hash(
        format!(
            "shared-loader-content-v2\0{}\0system-math\0{}\0module-api-v{}\0exports={}",
            profile.as_str(),
            shared_library_link_policy(),
            EMBEDDING_MODULE_API_VERSION,
            public_symbols.join(","),
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
    public_symbols: &[&str],
    workspace: &Path,
) -> Result<(), NativeError> {
    let identity = shared_library_link_identity(output);
    #[cfg(target_os = "macos")]
    {
        let exports = workspace.join("exports.txt");
        let source = public_symbols
            .iter()
            .map(|symbol| format!("_{symbol}\n"))
            .collect::<String>();
        materialize(&exports, source.as_bytes(), false)?;
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
        let source = format!(
            "KILN_1 {{\n  global:\n{}  local:\n    *;\n}};\n",
            public_symbols
                .iter()
                .map(|symbol| format!("    {symbol};\n"))
                .collect::<String>()
        );
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
    let _ = (options, identity, public_symbols, workspace);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_ir::{
        BYTECODE_VERSION, Constant, ExportBinding, FunctionProto, HostOp, ImportBinding,
        Instruction, ModuleProgram,
    };
    use std::sync::{Arc, Barrier};

    struct OxcFrontend;

    impl kiln_module_graph::Frontend for OxcFrontend {
        fn parse(&mut self, path: &str, source: &str) -> kiln_hir::Module {
            frontend_oxc::parse(path, source)
        }

        fn parse_commonjs(&mut self, path: &str, source: &str) -> kiln_module_graph::CommonJsParse {
            let (module, static_requires) =
                frontend_oxc::parse_commonjs_with_requires(path, source);
            kiln_module_graph::CommonJsParse {
                module,
                static_requires,
            }
        }
    }

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

    fn run_native_http_fixture(name: &str, source: &str) -> Option<String> {
        use kiln_module_resolver::NodeRuntimeResolver;
        use std::process::Command;

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native node:http fixture: no C compiler found");
            return None;
        };
        let project = TestDirectory::new(name);
        let entry = project.path.join("main.mjs");
        fs::write(&entry, source).expect("write node:http fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("source graph compiles");
        let report = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native node:http graph builds");
        let output = Command::new(&report.output)
            .output()
            .expect("run native node:http fixture");
        assert!(
            output.status.success(),
            "status={} stdout={:?} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Some(String::from_utf8(output.stdout).expect("fixture output is UTF-8"))
    }

    #[test]
    fn owned_result_wrapper_source_is_deterministic_and_has_no_static_result_state() {
        let source = owned_result_wrapper_source(
            "kiln_probe",
            "scalar",
            "kiln_probe(&proxy_host, &value)",
            "",
        );
        assert_eq!(
            source,
            owned_result_wrapper_source(
                "kiln_probe",
                "scalar",
                "kiln_probe(&proxy_host, &value)",
                "",
            )
        );
        for fragment in [
            "kiln_probe_api_v2",
            "kiln_module_api_v2",
            "kiln_result_v2",
            "kiln_v2_scalar_kiln_probe_run",
            "kiln_v2_scalar_kiln_probe_read",
            "kiln_v2_scalar_kiln_probe_dispose",
            "failure_status",
            "*out = NULL",
            "memcpy(copy, value.ptr, value.len)",
        ] {
            assert!(
                source.contains(fragment),
                "missing generated fragment: {fragment}"
            );
        }
        assert!(!source.contains("static kiln_result_v2"));
        assert!(!source.contains("static kiln_value result"));
    }

    fn environment_digest(kind: CompilerKind, entries: &[(&str, &str)]) -> Digest {
        let values = entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), std::ffi::OsString::from(value)))
            .collect::<std::collections::HashMap<_, _>>();
        compiler_environment_digest_with(kind, |name| values.get(name).cloned())
    }

    #[test]
    fn indexed_compiles_return_in_index_order_under_uneven_work() {
        let count = 64;
        let objects = compile_indexed(count, "test compile", |index| {
            // Deliberately uneven: the low indices are the slow ones, so a scheduler that
            // returned completion order rather than index order would say so loudly.
            if index % 8 == 0 {
                std::thread::sleep(Duration::from_millis(10 * (8 - index / 8) as u64));
            }
            Ok(index)
        })
        .expect("every index compiles");
        assert_eq!(objects, (0..count).collect::<Vec<_>>());
    }

    #[test]
    fn an_indexed_compile_reports_the_lowest_failing_index() {
        let failure = compile_indexed(32, "test compile", |index| {
            // The later failure is also the faster one, so whichever worker claims it will
            // usually report first. The lowest failing index must still win.
            if index == 3 {
                std::thread::sleep(Duration::from_millis(20));
            }
            if index == 3 || index == 17 {
                return Err(NativeError::NumberProvider {
                    message: format!("index {index}"),
                });
            }
            Ok(index)
        })
        .expect_err("a failing index fails the batch");
        match failure {
            NativeError::NumberProvider { message } => assert_eq!(message, "index 3"),
            other => panic!("unexpected failure: {other}"),
        }
    }

    #[test]
    fn a_panicking_indexed_worker_becomes_a_worker_panic() {
        let failure = compile_indexed(16, "panicking test compile", |index| {
            assert_ne!(index, 9, "deliberate test panic");
            Ok(index)
        })
        .expect_err("a panicking worker fails the batch");
        assert!(matches!(
            failure,
            NativeError::WorkerPanic {
                stage: "panicking test compile"
            }
        ));
    }

    #[test]
    fn one_slow_index_does_not_hold_the_indices_behind_it() {
        let workers = rayon::current_num_threads();
        if workers < 2 {
            // Nothing to steal from on a single-core machine.
            return;
        }
        let count = workers * 4;
        // Index zero blocks until every other index has finished. Under a static partition its
        // chunk-mates could never run, so this would time out; claiming indices one at a time lets
        // the other workers drain the whole rest of the space while zero waits.
        let progress = (std::sync::Mutex::new(0usize), std::sync::Condvar::new());
        let progress_ref = &progress;
        let observed = compile_indexed(count, "test compile", move |index| {
            let (finished, signal) = progress_ref;
            if index == 0 {
                let mut finished = finished.lock().expect("progress lock");
                while *finished < count - 1 {
                    let (guard, timeout) = signal
                        .wait_timeout(finished, Duration::from_secs(20))
                        .expect("progress lock");
                    finished = guard;
                    if timeout.timed_out() {
                        break;
                    }
                }
                return Ok(*finished);
            }
            *finished.lock().expect("progress lock") += 1;
            signal.notify_all();
            Ok(0)
        })
        .expect("every index compiles");
        assert_eq!(
            observed[0],
            count - 1,
            "every other index should finish while index zero is blocked"
        );
    }

    #[test]
    fn async_realm_contract_is_in_runtime_not_application_identity() {
        // Shape, not value. This was a pinned literal, and on 2026-08-31 alone it went 3 -> 5
        // -> 15 -> 17: three intended bumps, three red baselines, and not one accidental bump
        // caught. A tripwire that fires only on the change it is meant to permit is friction,
        // and the repair -- editing the literal -- teaches nothing and is indistinguishable
        // from rubber-stamping a mistake.
        //
        // What actually protects the caches is that the identity *has* every component, since a
        // missing one silently makes two different ABIs look alike. A bump is then visible in
        // the diff of the constant it comes from, where a reviewer can see what moved.
        let identity = module_abi_identity();
        for component in ["host-", "c-", "generator-"] {
            assert!(
                identity.contains(component),
                "module ABI identity {identity:?} has no `{component}` component; a missing one \
                 makes two different ABIs hash alike and every cached artifact is then suspect"
            );
        }
        assert!(runtime_abi_identity().contains("async-5"));
        // Same reasoning as above, and the same history: this pin was 14, then 15, then 18, and
        // is 23 as this is written -- all intended, none caught by being pinned.
        // What matters is that the provider's version reaches the identity at all: if it did not,
        // an archive built before an ABI change would keep being linked against emitted C that
        // expects the change, which is exactly the "undefined symbols from a stale archive"
        // failure this repository loses time to.
        let provider = native_number_abi_identity();
        let version = provider
            .rsplit_once('-')
            .map(|(_, version)| version)
            .unwrap_or_default();
        assert!(
            provider.starts_with("kiln-native-number-provider-") && version.parse::<u32>().is_ok(),
            "provider identity {provider:?} must end in the ABI version, or a stale archive and \
             a fresh one hash alike"
        );
    }

    #[test]
    fn live_cache_identity_catalogue_covers_every_stage_and_profile_family() {
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native cache identity catalogue: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-cache-identities");
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        let identities = builder
            .live_cache_identities()
            .expect("current native identities are enumerable");

        assert_eq!(
            identities.len(),
            34,
            "six generated-source families, one number provider, and nine stage families across three profiles"
        );

        let tool = builder.toolchain().identity();
        let target = target_identity(tool);
        let compiler = compiler_identity(tool);
        for profile in [
            CacheProfile::Debug,
            CacheProfile::Check,
            CacheProfile::Release,
        ] {
            for identity in [
                cache_identity(
                    compiler.clone(),
                    Backend::C,
                    &module_abi_identity(),
                    &target,
                    profile.clone(),
                ),
                cache_identity(
                    compiler.clone(),
                    Backend::Custom("native-runtime".to_owned()),
                    &runtime_abi_identity(),
                    &target,
                    profile.clone(),
                ),
                cache_identity(
                    compiler.clone(),
                    Backend::Custom("native-shared-graph".to_owned()),
                    &runtime_abi_identity(),
                    &target,
                    profile,
                ),
            ] {
                assert!(identities.contains(&identity));
            }
        }
    }

    #[test]
    fn typed_node_querystring_has_a_native_materializer() {
        use kiln_module_resolver::{NodeBuiltin, NodeRequestKind, PinnedPackageResolver};

        let project = TestDirectory::new("native-node-builtin-gate");
        let entry = project.path.join("main.mjs");
        fs::write(&entry, "export const marker = 1;\n").expect("write entry");
        let builtin_path = NodeBuiltin::QueryString.virtual_path(&project.path);
        let entry_program = ModuleProgram {
            program: Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: vec![Constant::Number(1_f64.to_bits())],
                code: vec![
                    Instruction::Const(0),
                    Instruction::StoreLocal(0),
                    Instruction::Const(0),
                    Instruction::Return,
                ],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: vec!["node:querystring".to_owned()],
            imports: Vec::new(),
            exports: vec![ExportBinding {
                exported: "marker".to_owned(),
                local_slot: 0,
            }],
            strictness: kiln_ir::Strictness::default(),
        };
        let provider_entry = fs::canonicalize(&entry).expect("canonical entry");
        let graph = ModuleGraph::build_with_typed_policy(
            &entry,
            PinnedPackageResolver::new_with_node_builtins_requests_and_formats(
                &project.path,
                [(
                    entry.clone(),
                    "node:querystring".to_owned(),
                    NodeRequestKind::Import,
                    builtin_path,
                )],
                [],
            )
            .expect("node resolver"),
            move |path, source| {
                assert_eq!(path, provider_entry);
                assert_eq!(source, ModuleSource::Source);
                Ok(kiln_module_graph::ModulePayload::Program(
                    entry_program.clone(),
                ))
            },
        )
        .expect("typed node graph");

        validate_native_node_builtins(&graph).expect("querystring materializes");
        let body = native_builtin_body(NodeBuiltin::QueryString).expect("materializer body");
        assert!(body.contains("KILN_C99_METHOD_NODE_QUERYSTRING_PARSE"));
        for export in ["decode", "default", "parse"] {
            assert!(native_builtin_supports_export(
                NodeBuiltin::QueryString,
                export
            ));
        }
    }

    #[test]
    fn native_node_querystring_parses_form_data_and_preserves_namespace_identity() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let root = TestDirectory::new("kiln-native-querystring");
        fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
            .expect("write package metadata");
        let entry = root.path.join("main.mjs");
        fs::write(
            &entry,
            r#"import querystring, { decode, parse } from "node:querystring";
const query = parse("hello=world&foo=b==ar&repeat=1&repeat=2&a+b=c+d&check=%E2%9C%93&bad=%ZZ&__proto__=safe");
const edges = decode("&&missing&=empty-key&empty-value=&short=%&half=%A&invalid=%C0&broken=%E2%28%A1");
console.log(parse === decode, querystring.parse === parse, query.hello, query.foo, query.repeat.join(","), query["a b"], query.check, query.bad, query.__proto__);
console.log(edges.missing === "", edges[""], edges["empty-value"] === "", edges.short, edges.half, edges.invalid, edges.broken);"#,
        )
        .expect("write querystring consumer");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build querystring graph");

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native querystring integration: no C compiler found");
            return;
        };
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        let run = builder
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native querystring graph builds and runs");
        assert_eq!(
            run.stdout,
            "true true world b==ar 1,2 c d ✓ %ZZ safe\ntrue empty-key true % %A � �(�\n".as_bytes()
        );
    }

    #[test]
    fn node_events_has_a_native_runtime_module_materializer() {
        use kiln_module_resolver::NodeBuiltin;

        let module = ModuleProgram {
            program: Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: Vec::new(),
                code: vec![Instruction::Return],
                local_count: 2,
                functions: Vec::new(),
            },
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: vec![
                ExportBinding {
                    exported: "EventEmitter".to_owned(),
                    local_slot: 0,
                },
                ExportBinding {
                    exported: "default".to_owned(),
                    local_slot: 1,
                },
            ],
            strictness: kiln_ir::Strictness::default(),
        };
        let node = kiln_module_graph::ModuleNode {
            path: PathBuf::from("/.kiln/builtins/events.builtin"),
            source: ModuleSource::NodeBuiltin(NodeBuiltin::Events),
            program: module,
            data: None,
            dependencies: Vec::new(),
        };
        let emitted = emit_native_graph_module(
            &node,
            ".kiln/builtins/events.builtin",
            &NativeRealmPolicy::default(),
        )
        .expect("events materializes");

        assert!(
            emitted
                .source
                .contains("KILN_C99_METHOD_NODE_EVENT_EMITTER")
        );
        assert!(emitted.source.contains("exports[0] = constructor;"));
        assert!(emitted.source.contains("exports[1] = constructor;"));
        assert!(native_builtin_body(NodeBuiltin::Https).is_some());
        assert!(native_builtin_body(NodeBuiltin::Path).is_some());
        assert!(native_builtin_supports_export(NodeBuiltin::Path, "join"));
    }

    #[test]
    fn node_net_has_one_native_runtime_module_materializer() {
        use kiln_module_resolver::NodeBuiltin;

        let body = native_builtin_body(NodeBuiltin::Net).expect("net materializes");
        assert!(body.contains("KILN_C99_METHOD_NODE_NET_CREATE_SERVER"));
        assert!(body.contains("exports[0] = connect;"));
        assert!(body.contains("exports[1] = connect;"));
        assert!(body.contains("exports[2] = create_server;"));
        assert!(native_builtin_body(NodeBuiltin::Https).is_some());
        assert!(native_builtin_body(NodeBuiltin::Path).is_some());
        assert!(native_builtin_supports_export(NodeBuiltin::Path, "resolve"));
    }

    #[test]
    fn node_stream_and_perf_hooks_have_native_runtime_module_materializers() {
        use kiln_module_resolver::NodeBuiltin;

        let stream = native_builtin_body(NodeBuiltin::Stream).expect("stream materializes");
        assert!(stream.contains("KILN_C99_METHOD_NODE_READABLE"));
        assert!(stream.contains("KILN_C99_METHOD_NODE_WRITABLE"));
        assert!(stream.contains("KILN_C99_METHOD_NODE_STREAM_FINISHED"));
        assert!(native_builtin_supports_export(
            NodeBuiltin::Stream,
            "addAbortSignal"
        ));

        let performance =
            native_builtin_body(NodeBuiltin::PerfHooks).expect("perf_hooks materializes");
        assert!(performance.contains("KILN_C99_METHOD_NODE_PERFORMANCE_NOW"));
        assert!(native_builtin_supports_export(
            NodeBuiltin::PerfHooks,
            "performance"
        ));
    }

    #[test]
    fn native_process_next_tick_forwards_variadic_arguments() {
        use kiln_module_resolver::NodeRuntimeResolver;
        use std::process::Command;

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native process.nextTick integration: no C compiler found");
            return;
        };
        let project = TestDirectory::new("kiln-native-process-next-tick");
        let entry = project.path.join("main.mjs");
        fs::write(
            &entry,
            r#"process.nextTick((left, right) => console.log(left + right), 19, 23);
"#,
        )
        .expect("write process.nextTick fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("source graph compiles");
        let report = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native process.nextTick graph builds");
        let output = Command::new(&report.output)
            .output()
            .expect("run native process.nextTick fixture");
        assert!(
            output.status.success(),
            "status={} stdout={:?} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"42\n");
    }

    #[test]
    fn native_node_fs_promises_read_write_stat_and_encoding() {
        use kiln_module_resolver::NodeRuntimeResolver;
        use std::process::Command;

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native node:fs/promises integration: no C compiler found");
            return;
        };
        let project = TestDirectory::new("kiln-native-node-fs-promises");
        let entry = project.path.join("main.mjs");
        let data = project.path.join("data.txt");
        let malformed = project.path.join("malformed.bin");
        let missing = project.path.join("missing.txt");
        let path = data.to_string_lossy();
        let malformed_path = malformed.to_string_lossy();
        let missing_path = missing.to_string_lossy();
        fs::write(
            &entry,
            format!(
                r#"import {{ readFile, stat, writeFile }} from 'node:fs/promises';
writeFile({path:?}, 'héllo').then(() => readFile({path:?}, 'utf8')).then((text) => {{
  console.log(text);
  return readFile({path:?});
}}).then((bytes) => {{
  console.log(Buffer.isBuffer(bytes));
  console.log(bytes.length);
  return stat({path:?});
}}).then((info) => {{
  console.log(info.size);
  console.log(info.isFile());
  console.log(info.isDirectory());
  console.log(info['@@kiln:node:is-file'] === undefined);
  Object.defineProperty(info, '@@kiln:node:is-file', {{ value: false }});
  console.log(info.isFile());
  delete info['@@kiln:node:is-file'];
  console.log(info.isFile());
  console.log(Object.hasOwn(info, '@@kiln:node:is-file') === false);
  console.log(('@@kiln:node:is-file' in info) === false);
  console.log(Object.getOwnPropertyDescriptor(info, '@@kiln:node:is-file') === undefined);
  return writeFile({malformed_path:?}, new Uint8Array([0xe2, 0x82]));
}}).then(() => readFile({malformed_path:?}, 'utf8')).then((text) => {{
  console.log(text === '\ufffd');
  return readFile({missing_path:?});
}}).then(() => console.log('unexpected'), (error) => {{
  console.log(error.code);
  console.log(error.syscall);
  console.log(error.path === {missing_path:?});
}});
"#,
                path = path,
                malformed_path = malformed_path,
                missing_path = missing_path
            ),
        )
        .expect("write node:fs/promises fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("source graph compiles");
        let report = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native node:fs/promises graph builds");
        let output = Command::new(&report.output)
            .output()
            .expect("run native node:fs/promises fixture");
        assert!(
            output.status.success(),
            "status={} stdout={:?} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout,
            "héllo\ntrue\n6\n6\ntrue\nfalse\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\nENOENT\nopen\ntrue\n".as_bytes()
        );
    }

    #[test]
    fn node_http_native_materializer_matches_the_authoritative_export_order() {
        use kiln_module_resolver::NodeBuiltin;

        // These indices were six slots wide and started at `Server`, which is the layout that
        // shipped and did not work: the authoritative list is eight names beginning
        // `IncomingMessage, METHODS, Server`, so every slot sat two low. `default` resolved to
        // `request` and `typeof http` answered "function" where node answers "object";
        // `createServer` resolved to `get`, so the first real server refused with "a function is
        // not a request options object". Node is the arbiter on both and the corrected layout
        // agrees with it, so the assertions below moved rather than the materializer.
        let body = native_builtin_body(NodeBuiltin::Http).expect("http materializes");
        assert!(body.contains("exports[0] = kiln_c99_undefined();"));
        assert!(body.contains("exports[1] = kiln_c99_undefined();"));
        assert!(body.contains("exports[2] = server;"));
        assert!(body.contains("exports[3] = server_response;"));
        assert!(body.contains("exports[4] = create_server;"));
        assert!(body.contains("exports[5] = namespace;"));
        assert!(body.contains("exports[6] = get;"));
        assert!(body.contains("exports[7] = request;"));
        assert!(body.contains("KILN_C99_METHOD_NODE_HTTP_SERVER"));
        assert!(body.contains("KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE"));
        assert!(body.contains("KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER"));
        assert!(native_builtin_supports_export(NodeBuiltin::Http, "Server"));
        assert!(native_builtin_supports_export(
            NodeBuiltin::Http,
            "ServerResponse"
        ));
    }

    #[test]
    fn native_node_http_exports_distinct_server_and_create_server_names() {
        let Some(output) = run_native_http_fixture(
            "kiln-native-node-http-callable-shape",
            r#"import http from "node:http";
if (typeof http.Server !== "function") throw new Error("Server is not callable");
if (typeof http.createServer !== "function") throw new Error("createServer is not callable");
if (http.Server === http.createServer) throw new Error("HTTP callables share identity");
if (http.Server.name !== "Server") throw new Error("wrong Server name");
if (http.createServer.name !== "createServer") throw new Error("wrong createServer name");
console.log("pass");
"#,
        ) else {
            return;
        };
        assert_eq!(output, "pass\n");
    }

    #[test]
    fn native_node_http_server_accepts_options_overload_without_listener() {
        let Some(output) = run_native_http_fixture(
            "kiln-native-node-http-options-overload",
            r#"import http from "node:http";
const optionOnly = new http.Server({ allowHalfOpen: true });
const nullOptions = new http.Server(null);
const withListener = new http.Server({}, () => {});
const factoryOptionOnly = http.createServer({});
const factoryWithListener = http.createServer({}, () => {});
for (const server of [optionOnly, nullOptions, withListener, factoryOptionOnly, factoryWithListener]) {
  if (typeof server.close !== "function") throw new Error("missing close");
  server.close();
}
console.log("pass");
"#,
        ) else {
            return;
        };
        assert_eq!(output, "pass\n");
    }

    #[test]
    fn native_node_http_server_exposes_timeout_policy_used_by_frameworks() {
        let Some(output) = run_native_http_fixture(
            "kiln-native-node-http-server-timeouts",
            r#"import http from "node:http";
const server = http.createServer(() => {});
console.log(server.timeout, server.keepAliveTimeout, server.requestTimeout);
server.setTimeout(12.9);
console.log(server.timeout);
console.log("pass");
"#,
        ) else {
            return;
        };
        assert_eq!(output, "0 5000 300000\n12\npass\n");
    }

    #[test]
    fn native_node_http_response_exposes_node_lifecycle_state() {
        let Some(output) = run_native_http_fixture(
            "kiln-native-node-http-response-state",
            r#"import http from "node:http";
const server = http.createServer((request, response) => {
  console.log(response.statusCode, response.headersSent, response.writableEnded, response.destroyed);
  response.statusCode = 201;
  response.writeHead(response.statusCode, { "content-type": "text/plain" });
  console.log(response.statusCode, response.headersSent, response.writableEnded);
  response.end("ok");
  console.log(response.headersSent, response.writableEnded);
  server.close();
});
server.listen(0, "127.0.0.1", () => {
  const request = http.request({ host: "127.0.0.1", port: server.address().port, path: "/" }, (response) => {
    response.on("data", () => {});
    response.on("end", () => {});
  });
  request.end();
});
"#,
        ) else {
            return;
        };
        assert_eq!(output, "200 false false false\n201 true false\ntrue true\n");
    }

    #[test]
    fn native_node_http_response_headers_are_case_insensitive_and_snapshot() {
        let Some(output) = run_native_http_fixture(
            "kiln-native-node-http-response-headers",
            r#"import http from "node:http";
const server = http.createServer((_request, response) => {
  response.setHeader("Content-Type", "text/plain");
  console.log(response.getHeader("content-type"), response.hasHeader("CONTENT-TYPE"));
  const headers = response.getHeaders();
  headers["content-type"] = "changed";
  console.log(response.getHeader("content-type"));
  response.removeHeader("CONTENT-TYPE");
  console.log(response.hasHeader("content-type"), response.getHeader("content-type"));
  response.end("ok");
  server.close();
});
server.listen(0, "127.0.0.1", () => {
  const request = http.request({ host: "127.0.0.1", port: server.address().port, path: "/" }, (response) => {
    response.on("data", () => {});
    response.on("end", () => {});
  });
  request.end();
});
"#,
        ) else {
            return;
        };
        assert_eq!(output, "text/plain true\ntext/plain\nfalse undefined\n");
    }

    #[test]
    fn native_node_http_incoming_message_exposes_request_lifecycle_state() {
        let Some(output) = run_native_http_fixture(
            "kiln-native-node-http-request-state",
            r#"import http from "node:http";
const server = http.createServer((request, response) => {
  console.log(request.httpVersion, request.httpVersionMajor, request.httpVersionMinor);
  console.log(request.aborted, request.complete, request.destroyed);
  request.on("end", () => {
    console.log(request.complete);
    response.end("ok");
    server.close();
  });
});
server.listen(0, "127.0.0.1", () => {
  const request = http.request({ host: "127.0.0.1", port: server.address().port, path: "/" }, (response) => {
    response.on("data", () => {});
    response.on("end", () => {});
  });
  request.end();
});
"#,
        ) else {
            return;
        };
        assert_eq!(output, "1.1 1 1\nfalse false false\ntrue\n");
    }

    #[test]
    fn native_node_http_server_rejects_invalid_options_with_a_node_code() {
        use kiln_module_resolver::NodeRuntimeResolver;
        use std::process::Command;

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native node:http option validation: no C compiler found");
            return;
        };
        let project = TestDirectory::new("kiln-native-node-http-invalid-options");
        let entry = project.path.join("main.mjs");
        fs::write(
            &entry,
            r#"import http from "node:http";
for (const option of ["foo", 42, true, []]) {
  let thrown;
  try { new http.Server(option); } catch (error) { thrown = error; }
  if (!(thrown instanceof TypeError)) throw new Error("missing TypeError");
  if (thrown.code !== "ERR_INVALID_ARG_TYPE") throw new Error("missing code");
}
console.log("pass");
"#,
        )
        .expect("write invalid-options fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("source graph compiles");
        let report = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native invalid-options graph builds");
        let output = Command::new(&report.output)
            .output()
            .expect("run native invalid-options fixture");
        assert!(
            output.status.success(),
            "status={} stdout={:?} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "pass\n");
    }

    #[test]
    fn native_node_net_source_graph_builds_through_the_production_frontend() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native node:net integration: no C compiler found");
            return;
        };
        let project = TestDirectory::new("kiln-native-node-net-source");
        let entry = project.path.join("main.mjs");
        fs::write(
            &entry,
            r#"import { createServer } from "node:net";
const server = createServer((socket) => {
  socket.on("data", (chunk) => {
    console.log(chunk);
    socket.end();
    server.close();
  });
});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
"#,
        )
        .expect("write node:net fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("source graph compiles");
        assert!(graph.modules().any(|node| matches!(
            node.source,
            ModuleSource::NodeBuiltin(kiln_module_resolver::NodeBuiltin::Net)
        )));
        let builder = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain);
        let report = builder
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("server"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native node:net graph builds");
        assert!(report.output.exists());

        use std::io::{BufRead, Read, Write};
        use std::net::TcpStream;
        use std::process::{Command, Stdio};
        let mut child = Command::new(&report.output)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start native node:net server");
        let stdout = child.stdout.take().expect("capture server stdout");
        let mut stdout = std::io::BufReader::new(stdout);
        let mut listening = String::new();
        stdout
            .read_line(&mut listening)
            .expect("read ephemeral listening port");
        let port: u16 = listening
            .trim()
            .parse()
            .expect("server prints numeric port");
        assert_ne!(port, 0);
        let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect loopback client");
        client
            .write_all(b"native loopback")
            .expect("write client bytes");
        client
            .shutdown(std::net::Shutdown::Write)
            .expect("finish client write");
        let mut ignored = Vec::new();
        client
            .read_to_end(&mut ignored)
            .expect("server closes client");
        let status = child.wait().expect("server exits after close");
        let mut rest = String::new();
        stdout
            .read_to_string(&mut rest)
            .expect("read callback output");
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture stderr")
            .read_to_string(&mut stderr)
            .expect("read stderr");
        assert!(status.success(), "server failed: {stderr}");
        assert_eq!(rest, "native loopback\n");
    }

    #[test]
    fn native_node_net_client_sends_final_bytes_and_reads_after_half_close() {
        use kiln_module_resolver::NodeRuntimeResolver;
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::process::Command;

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native node:net client integration: no C compiler found");
            return;
        };
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback server");
        let port = listener.local_addr().expect("read loopback address").port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept native client");
            let mut request = String::new();
            stream
                .read_to_string(&mut request)
                .expect("read through client EOF");
            assert_eq!(request, "final bytes");
            stream
                .write_all(b"reply")
                .expect("write reply after half-close");
        });
        let project = TestDirectory::new("kiln-native-node-net-client");
        let entry = project.path.join("main.mjs");
        fs::write(
            &entry,
            format!(
                r#"import {{ connect }} from "node:net";
const socket = connect({port}, "127.0.0.1", () => {{
  if (socket.readyState !== "open") throw new Error("socket was not open");
  socket.end("final bytes");
  if (socket.readyState !== "readOnly") throw new Error("socket was not read-only");
}});
let reply = "";
socket.setEncoding("utf8");
socket.on("data", chunk => reply += chunk);
socket.on("end", () => {{}});
socket.on("close", () => {{
  if (socket.readyState !== "closed") throw new Error("socket was not closed");
  console.log(reply);
}});
"#
            ),
        )
        .expect("write node:net client fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("source graph compiles");
        let report = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("client"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native node:net client builds");
        let output = Command::new(&report.output)
            .output()
            .expect("run native node:net client");
        server.join().expect("loopback server completes");
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"reply\n");
    }

    #[test]
    fn native_node_http_server_responds_after_node_net_client_half_close() {
        use kiln_module_resolver::NodeRuntimeResolver;
        use std::io::Read;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native HTTP half-close integration: no C compiler found");
            return;
        };
        let project = TestDirectory::new("kiln-native-http-half-close");
        let entry = project.path.join("main.mjs");
        fs::write(
            &entry,
            include_str!(
                "../../../conformance/node/upstream-adapted/http-server-allow-half-open.mjs"
            ),
        )
        .expect("write HTTP half-close fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("source graph compiles");
        let report = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("server"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native HTTP half-close fixture builds");
        let mut child = Command::new(&report.output)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run native HTTP half-close fixture");
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll native fixture") {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill stuck native fixture");
                break child.wait().expect("reap stuck native fixture");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut stdout = String::new();
        child
            .stdout
            .take()
            .expect("capture stdout")
            .read_to_string(&mut stdout)
            .expect("read stdout");
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture stderr")
            .read_to_string(&mut stderr)
            .expect("read stderr");
        assert!(
            status.success(),
            "status={status} stdout={stdout:?} stderr={stderr:?}"
        );
        assert_eq!(stdout, "pass\n");
    }

    #[test]
    fn native_node_http_server_handles_two_pipelined_requests_on_one_socket() {
        use kiln_module_resolver::NodeRuntimeResolver;
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native HTTP pipelining integration: no C compiler found");
            return;
        };
        let project = TestDirectory::new("kiln-native-http-pipelining");
        let reservation = TcpListener::bind(("127.0.0.1", 0)).expect("reserve loopback port");
        let port = reservation.local_addr().expect("read reserved port").port();
        drop(reservation);
        let entry = project.path.join("main.mjs");
        fs::write(
            &entry,
            format!(
                r#"import {{ createServer }} from "node:http";
let requests = 0;
const server = createServer((request, response) => {{
  console.log(request.url);
  response.end(request.url);
  requests += 1;
  if (requests === 2) server.close();
}});
server.listen({port}, "127.0.0.1");
"#
            ),
        )
        .expect("write HTTP pipelining fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("source graph compiles");
        let report = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("server"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native HTTP pipelining fixture builds");
        let mut child = Command::new(&report.output)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run native HTTP pipelining fixture");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut client = loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(client) => break client,
                Err(error) if Instant::now() < deadline => {
                    if let Some(status) = child.try_wait().expect("poll native server") {
                        let mut stderr = String::new();
                        child
                            .stderr
                            .take()
                            .expect("capture stderr")
                            .read_to_string(&mut stderr)
                            .expect("read stderr");
                        panic!("server exited before listen: {status}; stderr={stderr:?}");
                    }
                    let _ = error;
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    child.kill().expect("kill server stuck before listen");
                    child.wait().expect("reap server stuck before listen");
                    panic!("server did not listen before deadline: {error}");
                }
            }
        };
        client
            .set_nonblocking(true)
            .expect("make HTTP client nonblocking");
        client
            .write_all(
                b"GET /first HTTP/1.1\r\nHost: localhost\r\n\r\nGET /second HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .expect("write pipelined requests");
        let mut response = String::new();
        let mut chunk = [0_u8; 4096];
        let response_deadline = Instant::now() + Duration::from_secs(10);
        while !response.contains("/second") {
            match client.read(&mut chunk) {
                Ok(0) => break,
                Ok(length) => {
                    response.push_str(&String::from_utf8_lossy(&chunk[..length]));
                    if response.len() > 64 * 1024 {
                        child.kill().expect("kill server repeating response bytes");
                        child.wait().expect("reap server repeating response bytes");
                        let prefix = &response[..response.floor_char_boundary(512)];
                        panic!(
                            "HTTP bridge repeated response bytes; length={} prefix={prefix:?}",
                            response.len()
                        );
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= response_deadline {
                        child.kill().expect("kill server stuck during responses");
                        child.wait().expect("reap server stuck during responses");
                        panic!("timed out reading pipelined responses; partial={response:?}");
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    panic!("read both pipelined responses: {error}; partial response={response:?}")
                }
            }
        }
        let first = response.find("/first").unwrap_or_else(|| {
            let status = child
                .try_wait()
                .expect("poll server missing first response")
                .unwrap_or_else(|| {
                    child.kill().expect("kill server missing first response");
                    child.wait().expect("reap server missing first response")
                });
            let mut stderr = String::new();
            child
                .stderr
                .take()
                .expect("capture stderr")
                .read_to_string(&mut stderr)
                .expect("read stderr");
            panic!("first response body missing: {response:?}; status={status}; stderr={stderr:?}")
        });
        let second = response.find("/second").unwrap_or_else(|| {
            child.kill().expect("kill server missing second response");
            child.wait().expect("reap server missing second response");
            let mut stderr = String::new();
            child
                .stderr
                .take()
                .expect("capture stderr")
                .read_to_string(&mut stderr)
                .expect("read stderr");
            panic!("second response body missing: {response:?}; stderr={stderr:?}")
        });
        assert!(
            first < second,
            "responses preserve request order: {response:?}"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll native server exit") {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill server stuck after responses");
                break child.wait().expect("reap server stuck after responses");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut rest = String::new();
        child
            .stdout
            .take()
            .expect("capture stdout")
            .read_to_string(&mut rest)
            .expect("read stdout");
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture stderr")
            .read_to_string(&mut stderr)
            .expect("read stderr");
        assert!(
            status.success(),
            "status={status} stdout={rest:?} stderr={stderr:?}"
        );
        assert_eq!(rest, "/first\n/second\n");
    }

    #[test]
    fn native_node_http_source_graph_serves_one_live_tokio_request() {
        use kiln_module_resolver::NodeRuntimeResolver;
        use std::io::{BufRead, Read, Write};
        use std::net::TcpStream;
        use std::process::{Command, Stdio};

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native node:http integration: no C compiler found");
            return;
        };
        let project = TestDirectory::new("kiln-native-node-http-source");
        let entry = project.path.join("main.mjs");
        fs::write(
            &entry,
            r#"import { createServer } from "node:http";
const server = createServer((request, response) => {
  console.log(request.method, request.url, request.httpVersion);
  if (!request.socket || request.connection !== request.socket) throw new Error("request socket");
  if (response.req !== request) throw new Error("response request");
  if (response.socket !== request.socket || response.connection !== request.socket) throw new Error("response socket");
  let body = "";
  request.on("data", chunk => body += chunk);
  request.on("end", () => {
    console.log(body);
    response.setHeader("Content-Type", "text/plain");
    response.writeHead(201);
    response.write("echo:");
    response.end(body);
    server.close();
  });
});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
"#,
        )
        .expect("write node:http fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("source graph compiles");
        assert!(graph.modules().any(|node| matches!(
            node.source,
            ModuleSource::NodeBuiltin(kiln_module_resolver::NodeBuiltin::Http)
        )));
        let report = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("server"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native node:http graph builds");

        let mut child = Command::new(&report.output)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start native node:http server");
        let stdout = child.stdout.take().expect("capture server stdout");
        let mut stdout = std::io::BufReader::new(stdout);
        let mut listening = String::new();
        stdout
            .read_line(&mut listening)
            .expect("read ephemeral listening port");
        let port: u16 = listening
            .trim()
            .parse()
            .expect("server prints numeric port");
        assert_ne!(port, 0);

        let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect HTTP client");
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .expect("set HTTP client timeout");
        client
            .write_all(
                b"POST /echo HTTP/1.1\r\nHost: localhost\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabc",
            )
            .expect("write first HTTP request fragment");
        client
            .write_all(b"123")
            .expect("write second HTTP request fragment");
        client
            .shutdown(std::net::Shutdown::Write)
            .expect("finish HTTP request");
        let mut response = String::new();
        let read_result = client.read_to_string(&mut response);
        if let Err(error) = read_result {
            let status = child.wait().expect("server exits after failed HTTP read");
            let mut stderr = String::new();
            child
                .stderr
                .take()
                .expect("capture stderr")
                .read_to_string(&mut stderr)
                .expect("read stderr");
            panic!(
                "HTTP read failed: {error}; status={status}; response={response:?}; stderr={stderr:?}"
            );
        }
        assert!(
            response.starts_with("HTTP/1.1 201 Created\r\n"),
            "{response:?}"
        );
        let response_lower = response.to_ascii_lowercase();
        assert!(
            response_lower.contains("content-type: text/plain\r\n"),
            "{response:?}"
        );
        assert!(
            response_lower.contains("connection: close\r\n"),
            "{response:?}"
        );
        assert!(
            response.ends_with("\r\n5\r\necho:\r\n6\r\nabc123\r\n0\r\n\r\n"),
            "{response:?}"
        );

        let status = child
            .wait()
            .expect("server exits after listener and socket close");
        let mut rest = String::new();
        stdout
            .read_to_string(&mut rest)
            .expect("read request callback output");
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture stderr")
            .read_to_string(&mut stderr)
            .expect("read stderr");
        assert!(status.success(), "server failed: {stderr}");
        assert_eq!(rest, "POST /echo 1.1\nabc123\n");
    }

    #[test]
    #[ignore = "native Fastify constructor still reaches an unowned Call refusal"]
    fn native_fastify_server_case_runs_through_the_production_graph_when_installed() {
        use kiln_module_resolver::NodeRuntimeResolver;
        use std::io::Read;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native Fastify integration: no C compiler found");
            return;
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/server/packages");
        if !root.join("node_modules/fastify/fastify.js").is_file() {
            eprintln!("skipping native Fastify integration: packages are not installed");
            return;
        }
        let entry = root.join("s04-fastify.mjs");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root).expect("Node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("Fastify source graph compiles");
        let project = TestDirectory::new("kiln-native-fastify");
        let report = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("server"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native Fastify graph builds");
        let mut child = Command::new(&report.output)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run native Fastify graph");
        let deadline = Instant::now() + Duration::from_secs(15);
        let (status, timed_out) = loop {
            if let Some(status) = child.try_wait().expect("poll native Fastify graph") {
                break (status, false);
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill stuck native Fastify graph");
                break (child.wait().expect("reap stuck native Fastify graph"), true);
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut stdout = String::new();
        child
            .stdout
            .take()
            .expect("capture Fastify stdout")
            .read_to_string(&mut stdout)
            .expect("read Fastify stdout");
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture Fastify stderr")
            .read_to_string(&mut stderr)
            .expect("read Fastify stderr");
        assert!(
            !timed_out,
            "native Fastify graph timed out; stdout={stdout:?} stderr={stderr:?}"
        );
        assert!(
            status.success(),
            "status={status}; stdout={stdout:?}; stderr={stderr:?}"
        );
        assert_eq!(
            stdout,
            "200 {\"ok\":true}\n200 {\"id\":\"7\"}\n200 {\"got\":{\"hello\":\"kiln\"}}\n404 {\"error\":\"not found\"}\n"
        );
        assert!(stderr.is_empty(), "stderr={stderr:?}");
    }

    /// Serving a request and *reading the reply* are two different claims, and the corpus only
    /// covered the first. `setEncoding` is the seam: it is the one method name a `net.Socket`, an
    /// `http.IncomingMessage` and a `stream.Readable` all own, so the compiled dispatch has to
    /// pick a handler from the receiver rather than from a category. When it picked by category
    /// instead, every `res.setEncoding("utf8")` threw `a object is not a Socket` -- with the whole
    /// live-request suite still green, because none of it ever read a response as text.
    ///
    /// The assertion is node's observable, not the absence of a throw: after `setEncoding("utf8")`
    /// a `data` chunk is a string, and `typeof` says so.
    #[test]
    fn native_node_http_client_response_set_encoding_delivers_text_chunks() {
        use kiln_module_resolver::NodeRuntimeResolver;
        use std::process::Command;

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native setEncoding integration: no C compiler found");
            return;
        };
        let project = TestDirectory::new("kiln-native-http-client-set-encoding");
        let entry = project.path.join("main.mjs");
        fs::write(
            &entry,
            r#"import http from "node:http";
const server = http.createServer((request, response) => {
  response.writeHead(200, { "content-type": "text/plain" });
  response.end("kiln");
});
server.listen(0, "127.0.0.1", () => {
  const { port } = server.address();
  http.get({ host: "127.0.0.1", port, path: "/" }, (response) => {
    response.setEncoding("utf8");
    let body = "";
    let kinds = "";
    response.on("data", chunk => { kinds += typeof chunk; body += chunk; });
    response.on("end", () => {
      console.log(response.statusCode, response.headers["content-type"], kinds, body);
      server.close();
    });
  });
});
"#,
        )
        .expect("write node:http setEncoding fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("source graph compiles");
        let report = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("client"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native node:http setEncoding graph builds");
        let output = Command::new(&report.output)
            .output()
            .expect("run native node:http setEncoding program");
        assert!(
            output.status.success(),
            "status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        // Recorded from node v25: `200 text/plain string kiln`.
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "200 text/plain string kiln\n"
        );
    }

    #[test]
    fn native_node_http_server_close_waits_for_delayed_response_after_half_close() {
        use kiln_module_resolver::NodeRuntimeResolver;
        use std::io::{BufRead, Read, Write};
        use std::net::TcpStream;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native HTTP delayed-close integration: no C compiler found");
            return;
        };
        let project = TestDirectory::new("kiln-native-http-delayed-close");
        let entry = project.path.join("main.mjs");
        fs::write(
            &entry,
            r#"import { createServer } from "node:http";
let socket_closes = 0;
const server = createServer((request, response) => {
  console.log("request");
  request.socket.on("close", () => {
    socket_closes += 1;
    console.log(`socket-close:${socket_closes}`);
  });
  setTimeout(() => { console.log("timer"); response.end("delayed"); }, 0);
  console.log("before-close");
  server.close(() => console.log("server-close"));
  console.log("after-close", request.socket.readyState);
});
server.httpAllowHalfOpen = true;
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
"#,
        )
        .expect("write delayed-close fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("source graph compiles");
        let report = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("server"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native delayed-close fixture builds");
        let mut child = Command::new(&report.output)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run native delayed-close fixture");
        let stdout = child.stdout.take().expect("capture server stdout");
        let mut stdout = std::io::BufReader::new(stdout);
        let mut listening = String::new();
        stdout
            .read_line(&mut listening)
            .expect("read delayed-close listening port");
        let port: u16 = listening
            .trim()
            .parse()
            .expect("server prints numeric port");
        let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect HTTP client");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set HTTP client timeout");
        client
            .write_all(b"GET /delayed HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .expect("write HTTP request");
        client
            .shutdown(std::net::Shutdown::Write)
            .expect("half-close HTTP request");
        let mut response = String::new();
        if let Err(error) = client.read_to_string(&mut response) {
            let _ = child.kill();
            let status = child
                .wait()
                .expect("reap delayed-close server after read failure");
            panic!("read delayed HTTP response: {error}; status={status}; response={response:?}");
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll delayed-close server exit") {
                break status;
            }
            if Instant::now() >= deadline {
                child
                    .kill()
                    .expect("kill delayed-close server after timeout");
                break child
                    .wait()
                    .expect("reap delayed-close server after timeout");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut rest = String::new();
        stdout
            .read_to_string(&mut rest)
            .expect("read delayed-close callback output");
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture server stderr")
            .read_to_string(&mut stderr)
            .expect("read server stderr");
        assert!(
            response.starts_with("HTTP/1.1 200 OK\r\n"),
            "response={response:?} status={status} stdout={rest:?} stderr={stderr:?}"
        );
        assert!(
            response.ends_with("\r\n7\r\ndelayed\r\n0\r\n\r\n"),
            "response={response:?} status={status} stdout={rest:?} stderr={stderr:?}"
        );
        assert!(status.success(), "server failed: {stderr}");
        assert_eq!(rest.matches("socket-close:").count(), 1, "stdout={rest:?}");
        assert_eq!(
            rest,
            "request\nbefore-close\nafter-close open\ntimer\nsocket-close:1\nserver-close\n"
        );
    }

    #[test]
    fn native_node_http_socket_destroy_uses_abort_close() {
        use kiln_module_resolver::NodeRuntimeResolver;
        use std::io::{BufRead, Read, Write};
        use std::net::TcpStream;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native HTTP destroy integration: no C compiler found");
            return;
        };
        let project = TestDirectory::new("kiln-native-http-destroy");
        let entry = project.path.join("main.mjs");
        fs::write(
            &entry,
            r#"import { createServer } from "node:http";
const server = createServer((request, response) => {
  console.log("destroy");
  request.socket.destroy();
  server.close(() => console.log("closed"));
});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
"#,
        )
        .expect("write HTTP destroy fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("source graph compiles");
        let report = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("server"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native HTTP destroy fixture builds");
        let mut child = Command::new(&report.output)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run native HTTP destroy fixture");
        let stdout = child.stdout.take().expect("capture destroy stdout");
        let mut stdout = std::io::BufReader::new(stdout);
        let mut listening = String::new();
        stdout
            .read_line(&mut listening)
            .expect("read destroy listening port");
        let port: u16 = listening.trim().parse().expect("numeric destroy port");
        let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect HTTP client");
        client
            .write_all(b"GET /destroy HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .expect("write destroy request");
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll destroy server") {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill destroy server after timeout");
                break child.wait().expect("reap destroy server after timeout");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut rest = String::new();
        stdout
            .read_to_string(&mut rest)
            .expect("read destroy callback output");
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture destroy stderr")
            .read_to_string(&mut stderr)
            .expect("read destroy stderr");
        assert!(
            status.success(),
            "status={status} stdout={rest:?} stderr={stderr:?}"
        );
        assert_eq!(rest, "destroy\nclosed\n");
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
                source: "".into(),
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
            strictness: kiln_ir::Strictness::default(),
        };
        ModuleGraph::build_with_modules(project.join("main.js"), project, move |_| module.clone())
            .expect("construct one-module graph")
    }

    fn realm_scheduling_graph(project: &Path) -> ModuleGraph {
        fs::create_dir_all(project).expect("create scheduling graph project");
        for name in ["dep.js", "main.js"] {
            fs::write(project.join(name), "").expect("write scheduling graph module");
        }
        let dependency = ModuleProgram {
            program: Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: vec![
                    Constant::String("dependency microtask".to_owned()),
                    Constant::String("dependency timer".to_owned()),
                    Constant::Number(0.0_f64.to_bits()),
                    Constant::Undefined,
                ],
                code: vec![
                    Instruction::LoadBuiltin(kiln_ir::Builtin::QueueMicrotask),
                    Instruction::MakeFunction(1),
                    Instruction::Call { argc: 1 },
                    Instruction::Pop,
                    Instruction::LoadBuiltin(kiln_ir::Builtin::SetTimeout),
                    Instruction::MakeFunction(0),
                    Instruction::Const(2),
                    Instruction::Call { argc: 2 },
                    Instruction::Pop,
                    Instruction::Const(3),
                    Instruction::Return,
                ],
                local_count: 0,
                functions: vec![
                    FunctionProto {
                        span: kiln_ir::Span::default(),
                        prologue_end: 0,
                        asynchronous: false,
                        name: None,
                        arrow: false,
                        generator: false,
                        async_generator: false,
                        parameter_count: 0,
                        declared_length: 0,
                        mapped_arguments: false,
                        local_count: 0,
                        code: vec![
                            Instruction::Const(1),
                            Instruction::HostCall {
                                op: HostOp::WriteStdout,
                                argc: 1,
                            },
                            Instruction::Return,
                        ],
                        upvalues: Vec::new(),
                    },
                    FunctionProto {
                        span: kiln_ir::Span::default(),
                        prologue_end: 0,
                        asynchronous: false,
                        name: None,
                        arrow: false,
                        generator: false,
                        async_generator: false,
                        parameter_count: 0,
                        declared_length: 0,
                        mapped_arguments: false,
                        local_count: 0,
                        code: vec![
                            Instruction::Const(0),
                            Instruction::HostCall {
                                op: HostOp::WriteStdout,
                                argc: 1,
                            },
                            Instruction::Return,
                        ],
                        upvalues: Vec::new(),
                    },
                ],
            },
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            strictness: kiln_ir::Strictness::default(),
        };
        let entry = ModuleProgram {
            program: Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: vec![
                    Constant::String("entry body".to_owned()),
                    Constant::Undefined,
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
            },
            dependencies: vec!["./dep.js".to_owned()],
            imports: Vec::new(),
            exports: Vec::new(),
            strictness: kiln_ir::Strictness::default(),
        };
        let dependency_path =
            fs::canonicalize(project.join("dep.js")).expect("canonical dependency");
        ModuleGraph::build_with_modules(project.join("main.js"), project, move |path| {
            if path == dependency_path {
                dependency.clone()
            } else {
                entry.clone()
            }
        })
        .expect("construct scheduling graph")
    }

    #[test]
    fn one_realm_defers_dependency_work_until_the_complete_native_graph_finishes() {
        let root = TestDirectory::new("kiln-native-realm-graph");
        let graph = realm_scheduling_graph(&root.path.join("project"));
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native Realm graph integration: no C compiler found");
            return;
        };
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        let run = builder
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native scheduling graph runs");
        assert_eq!(
            run.stdout,
            b"entry body\ndependency microtask\ndependency timer\n"
        );
    }

    /// A compiled binary is a single-executable application in node's sense, so it reports the
    /// command line the way node does for one: `argv[0]` and `argv[1]` both name the executable,
    /// and the program's own arguments start at index 2. That is not a cosmetic choice -- every
    /// argument-reading program, and `const [, , first] = process.argv`, counts from there, and a
    /// one-element argv would silently hand each of them the wrong argument.
    ///
    /// Run through `Command` rather than `run_graph`, because the whole question is what the OS
    /// put in `main`'s `argv` and there is no other way to put anything there.
    #[test]
    fn native_process_argv_reports_the_single_executable_command_line() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native process.argv integration: no C compiler found");
            return;
        };
        let project = TestDirectory::new("kiln-native-process-argv");
        let entry = project.path.join("main.mjs");
        fs::write(
            &entry,
            r#"console.log(JSON.stringify([
  process.argv.length,
  process.argv[0] === process.argv[1],
  process.execPath === process.argv[0],
  process.argv.slice(2),
]));
"#,
        )
        .expect("write process.argv fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("process.argv source compiles");
        let output = project.path.join("program");
        NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .build_graph(
                &graph,
                &NativeBuildOptions {
                    output: output.clone(),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("process.argv graph builds natively");

        let bare = std::process::Command::new(&output)
            .output()
            .expect("run the compiled program");
        assert_eq!(bare.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&bare.stdout),
            "[2,true,true,[]]\n",
            "stderr: {}",
            String::from_utf8_lossy(&bare.stderr)
        );

        let passed = std::process::Command::new(&output)
            .args(["a", "b", "c"])
            .output()
            .expect("run the compiled program with arguments");
        assert_eq!(passed.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&passed.stdout),
            "[5,true,true,[\"a\",\"b\",\"c\"]]\n",
            "stderr: {}",
            String::from_utf8_lossy(&passed.stderr)
        );
    }

    #[test]
    fn native_process_next_tick_uses_the_realm_lane_before_microtasks() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native process.nextTick integration: no C compiler found");
            return;
        };
        let project = TestDirectory::new("kiln-native-process-next-tick");
        let entry = project.path.join("main.mjs");
        fs::write(
            &entry,
            r#"const order = [];
const nextTick = process.nextTick;
order.push(`${delete process.nextTick}:${process.nextTick === undefined}`);
Promise.resolve().then(function () { order.push("promise"); });
nextTick(function (first, second) {
  order.push(`${first}:${second}:${this === undefined}`);
  nextTick(function () { order.push("nested"); });
  queueMicrotask(function () { order.push("micro-from-tick"); });
}, "a", 2);
queueMicrotask(function () { order.push("micro"); });
setTimeout(function () { console.log(order.join(",")); }, 0);
"#,
        )
        .expect("write process.nextTick fixture");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("process.nextTick source compiles");
        let run = NativeBuilder::new(ArtifactCache::new(project.path.join("cache")), toolchain)
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: project.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("process.nextTick graph runs natively");
        assert_eq!(run.exit_code, Some(0));
        assert_eq!(
            run.stdout,
            b"true:true,a:2:true,nested,promise,micro,micro-from-tick\n"
        );
        assert!(run.stderr.is_empty());
    }

    #[test]
    fn embedded_graph_owns_one_realm_through_async_quiescence() {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            eprintln!("skipping embedded Realm graph integration on unsupported platform");
            return;
        }
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping embedded Realm graph integration: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-embedded-realm-graph");
        let graph = realm_scheduling_graph(&root.path.join("project"));
        let builder = NativeBuilder::new(
            ArtifactCache::new(root.path.join("cache")),
            toolchain.clone(),
        );
        let library = builder
            .build_shared_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join(if cfg!(target_os = "macos") {
                        "program.dylib"
                    } else {
                        "program.so"
                    }),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("build embedded scheduling graph");
        let package = shared_publication::read_shared_library_package(&library.output)
            .expect("read graph shared-library package");
        assert_eq!(
            package.manifest().legacy_entry_symbol,
            library.module_symbol
        );
        assert_eq!(package.manifest().v2_api_symbol, library.module_api_symbol);
        let header_name = library
            .header
            .file_name()
            .and_then(|name| name.to_str())
            .expect("shared graph header name");
        let host_source = root.path.join("host.c");
        fs::write(
            &host_source,
            format!(
                r#"#include "{header_name}"
#include <stdio.h>

static int write_host_call(void *context, uint16_t operation, uint16_t argc,
                           const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != KILN_HOST_OP_WRITE_STDOUT || argc != 1 || arguments == NULL ||
        result == NULL || arguments[0].tag != KILN_VALUE_STRING ||
        (arguments[0].len != 0 && arguments[0].ptr == NULL)) return KILN_C99_HOST_ERROR;
    if (fwrite(arguments[0].ptr, 1, arguments[0].len, stdout) != arguments[0].len ||
        fputc('\n', stdout) == EOF) return KILN_C99_HOST_ERROR;
    *result = (kiln_value){{ KILN_VALUE_UNDEFINED, 0, {{0}} }};
    return KILN_C99_OK;
}}

int main(void) {{
    kiln_host host = {{ write_host_call, NULL }};
    kiln_value result;
    return {symbol}(&host, &result) == KILN_C99_OK ? 0 : 1;
}}
"#,
                symbol = library.module_symbol,
            ),
        )
        .expect("write embedded scheduling host");
        let host_object = root.path.join("host.o");
        let mut compile = CompileOptions::default();
        compile.include_dirs.push(root.path.clone());
        toolchain
            .compile_c(&host_source, &host_object, &compile)
            .expect("compile embedded scheduling host");
        let host_executable = root.path.join("host");
        let loader_directory = library.loader.parent().expect("loader directory");
        let mut link = LinkOptions::default();
        link.runtime_libraries.push(library.output.clone());
        link.extra_args
            .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
        toolchain
            .link_executable(&[host_object], &host_executable, &link)
            .expect("link embedded scheduling host");
        let mut command = Command::new(host_executable);
        #[cfg(target_os = "linux")]
        command.env("LD_LIBRARY_PATH", loader_directory);
        #[cfg(target_os = "macos")]
        command.env("DYLD_LIBRARY_PATH", loader_directory);
        let output = command.output().expect("run embedded scheduling graph");
        assert!(
            output.status.success(),
            "embedded scheduling graph failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout,
            b"entry body\ndependency microtask\ndependency timer\n"
        );
    }

    fn star_reexport_graph(project: &Path) -> ModuleGraph {
        fs::create_dir_all(project).expect("create star graph project");
        for name in ["origin.js", "barrel.js", "main.js"] {
            fs::write(project.join(name), "").expect("write star graph module");
        }
        let origin = ModuleProgram {
            program: Program {
                source: "".into(),
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
            strictness: kiln_ir::Strictness::default(),
        };
        let barrel = ModuleProgram {
            program: Program {
                source: "".into(),
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
            strictness: kiln_ir::Strictness::default(),
        };
        let entry = ModuleProgram {
            program: Program {
                source: "".into(),
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
            strictness: kiln_ir::Strictness::default(),
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
                source: "".into(),
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
            strictness: kiln_ir::Strictness::default(),
        };
        let entry = ModuleProgram {
            program: Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: vec![Constant::String("answer".into())],
                code: vec![
                    Instruction::LoadLocal(0),
                    Instruction::GetProperty(0),
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 1,
                    },
                    Instruction::Return,
                ],
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
            strictness: kiln_ir::Strictness::default(),
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
        assert_eq!(launcher.matches("kiln_realm_create(&realm)").count(), 1);
        assert_eq!(launcher.matches("kiln_realm_destroy(realm)").count(), 1);
        assert_eq!(
            launcher
                .matches("kiln_realm_enter(realm, kiln_graph_enter, &graph)")
                .count(),
            1
        );
        assert!(!launcher.contains("kiln_graph_enter_module"));
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
                    realm: NativeRealmPolicy::default(),
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
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native shared star graph links");
    }

    #[test]
    fn native_graph_reserves_commonjs_wrapper_bindings_for_the_launcher() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let root = TestDirectory::new("kiln-native-commonjs-wrapper");
        fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
            .expect("write root package");
        fs::create_dir(root.path.join("pkg")).expect("create package directory");
        fs::write(root.path.join("pkg/package.json"), r#"{"type":"commonjs"}"#)
            .expect("write CommonJS package");
        fs::write(
            root.path.join("pkg/index.js"),
            "console.log(__filename, __dirname); module.exports = 42;",
        )
        .expect("write CommonJS module");
        fs::write(root.path.join("pkg/dep.js"), "module.exports = 41;")
            .expect("write CommonJS dependency");
        let entry = root.path.join("main.mjs");
        fs::write(
            &entry,
            "import answer from './pkg/index.js'; console.log(answer);",
        )
        .expect("write entry");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build mixed graph");
        let modules = graph
            .modules()
            .map(|node| {
                (
                    node,
                    emit_native_graph_module(
                        node,
                        &logical_module_path(graph.project_root(), &node.path)
                            .expect("logical path"),
                        &NativeRealmPolicy::default(),
                    )
                    .expect("module emits"),
                )
            })
            .collect::<Vec<_>>();

        let launcher = graph_launcher_source(&graph, &modules).expect("launcher reserves wrappers");
        let commonjs_index = modules
            .iter()
            .position(|(node, _)| node.source == ModuleSource::CommonJs)
            .expect("CommonJS module");
        let commonjs_path = modules[commonjs_index].0.path.to_string_lossy();
        let commonjs_dir = modules[commonjs_index]
            .0
            .path
            .parent()
            .expect("CommonJS module has a parent")
            .to_string_lossy();
        let logical_commonjs =
            logical_module_path(graph.project_root(), &modules[commonjs_index].0.path)
                .expect("logical CommonJS path");
        assert!(launcher.contains("static kiln_value kiln_imports_0[5];"));
        assert!(launcher.contains("_commonjs_wrapper(kiln_imports_0, (size_t)5, "));
        assert!(launcher.contains(&format!(
            "{}_commonjs_wrapper(kiln_imports_{commonjs_index}, (size_t)5, {}, (size_t){}, {}, (size_t){}",
            modules[commonjs_index].1.symbol,
            native_c_string(&commonjs_path),
            commonjs_path.len(),
            native_c_string(&commonjs_dir),
            commonjs_dir.len(),
        )));
        assert!(launcher.contains(&format!(
            "kiln_graph_diagnostic(\"commonjs-wrapper\", {})",
            native_c_string(&logical_commonjs)
        )));
        assert_eq!(
            launcher.matches("_commonjs_wrapper(kiln_imports_").count(),
            1
        );
        let wrapper = graph_shared_wrapper_source(&graph, &modules, "kiln_commonjs_graph")
            .expect("shared wrapper emits CommonJS support");
        let context = wrapper
            .find("typedef struct kiln_commonjs_context")
            .expect("shared wrapper declares CommonJS context");
        let loader = wrapper
            .find("static int kiln_graph_load_commonjs")
            .expect("shared wrapper declares CommonJS loader");
        assert!(context < loader, "context must precede its first use");
        assert!(wrapper.contains(&native_c_string(&commonjs_path)));
        assert!(wrapper.contains(&native_c_string(&commonjs_dir)));

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native CommonJS execution: no C compiler found");
            return;
        };
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        let report = builder
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native mixed graph builds and runs");
        assert_eq!(
            report.stdout,
            format!("{commonjs_path} {commonjs_dir}\n42\n").as_bytes()
        );
        builder
            .build_shared_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join(shared_library_name()),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native shared CommonJS graph builds and links");

        fs::write(
            root.path.join("pkg/index.js"),
            "module.exports = require('./dep.js') + 1;",
        )
        .expect("rewrite CommonJS module with require");
        let require_graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build CommonJS require graph");
        let required = builder
            .run_graph(
                &require_graph,
                &NativeBuildOptions {
                    output: root.path.join("required-program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native CommonJS require graph builds and runs");
        assert_eq!(required.stdout, b"42\n");

        fs::write(
            root.path.join("pkg/index.js"),
            "const first = require('./dep.js'); const second = require('./dep.js'); console.log(first === second, first.loads); module.exports = 42;",
        )
        .expect("rewrite CommonJS module with repeated require");
        fs::write(
            root.path.join("pkg/dep.js"),
            "globalThis.__kilnLoads = (globalThis.__kilnLoads || 0) + 1; module.exports = { loads: globalThis.__kilnLoads };",
        )
        .expect("write stateful CommonJS dependency");
        let cached_graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build repeated CommonJS require graph");
        let cached = builder
            .run_graph(
                &cached_graph,
                &NativeBuildOptions {
                    output: root.path.join("cached-program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native repeated CommonJS require graph builds and runs");
        assert_eq!(cached.stdout, b"true 1\n42\n");
    }

    #[test]
    fn native_commonjs_require_receives_the_builtin_projection_on_both_graph_paths() {
        use kiln_module_resolver::{NodeBuiltin, NodeRuntimeResolver};

        let root = TestDirectory::new("kiln-native-commonjs-builtin-projection");
        fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
            .expect("write root package");
        fs::create_dir(root.path.join("pkg")).expect("create package directory");
        fs::write(root.path.join("pkg/package.json"), r#"{"type":"commonjs"}"#)
            .expect("write CommonJS package");
        fs::write(
            root.path.join("pkg/index.js"),
            "const { format } = require('node:util'); module.exports = format('%s:%d', 'ok', 7);",
        )
        .expect("write CommonJS builtin consumer");
        let entry = root.path.join("main.mjs");
        fs::write(
            &entry,
            "import answer from './pkg/index.js'; console.log(answer);",
        )
        .expect("write entry");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build CommonJS builtin graph");
        let modules = graph
            .modules()
            .map(|node| {
                (
                    node,
                    emit_native_graph_module(
                        node,
                        &logical_module_path(graph.project_root(), &node.path)
                            .expect("logical path"),
                        &NativeRealmPolicy::default(),
                    )
                    .expect("module emits"),
                )
            })
            .collect::<Vec<_>>();
        let util_index = modules
            .iter()
            .position(|(node, _)| node.source == ModuleSource::NodeBuiltin(NodeBuiltin::Util))
            .expect("util module");
        let commonjs_index = modules
            .iter()
            .position(|(node, _)| node.source == ModuleSource::CommonJs)
            .expect("CommonJS module");
        let default_index = native_commonjs_projection_index(modules[util_index].0)
            .expect("builtin CommonJS projection");
        let expected_standalone =
            format!("*result = kiln_exports_{util_index}[{default_index}]; return 0;");
        let expected_shared =
            format!("*result = graph->exports[{util_index}][{default_index}]; return KILN_C99_OK;");
        let launcher = graph_launcher_source(&graph, &modules).expect("launcher emits");
        assert!(launcher.contains(&expected_standalone));
        let wrapper = graph_shared_wrapper_source(&graph, &modules, "kiln_builtin_projection")
            .expect("shared wrapper emits");
        assert!(wrapper.contains(&expected_shared));
        assert!(launcher.contains(&format!("kiln_commonjs_require_{commonjs_index}")));

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native builtin projection integration: no C compiler found");
            return;
        };
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        let run = builder
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("standalone graph runs");
        assert_eq!(run.stdout, b"ok:7\n");
        builder
            .build_shared_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join(shared_library_name()),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("shared graph builds");
    }

    #[test]
    fn native_commonjs_callable_exports_cross_module_boundaries() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let root = TestDirectory::new("kiln-native-commonjs-callable-export");
        fs::write(root.path.join("package.json"), r#"{"type":"commonjs"}"#)
            .expect("write package metadata");
        fs::write(
            root.path.join("callee.js"),
            "module.exports = function answer(value) { return value + 1 }",
        )
        .expect("write callable CommonJS provider");
        let entry = root.path.join("main.js");
        fs::write(
            &entry,
            "const answer = require('./callee.js'); console.log(answer(41)); module.exports = answer(1);",
        )
        .expect("write callable CommonJS consumer");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build CommonJS callable graph");
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native CommonJS callable integration: no C compiler found");
            return;
        };
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        let report = builder
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native CommonJS callable graph builds and runs");
        assert_eq!(
            report.exit_code,
            Some(0),
            "stderr={}",
            String::from_utf8_lossy(&report.stderr)
        );
        assert_eq!(report.stdout, b"42\n");
    }

    #[test]
    fn native_runtime_callable_properties_are_realm_wide_across_modules() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let root = TestDirectory::new("kiln-native-realm-callable-properties");
        fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
            .expect("write package metadata");
        fs::write(
            root.path.join("writer.mjs"),
            r#"import { Buffer } from "node:buffer";
Buffer.realmMarker = { answer: 42 };
export { Buffer };"#,
        )
        .expect("write callable-property producer");
        let entry = root.path.join("main.mjs");
        fs::write(
            &entry,
            r#"import { Buffer as written } from "./writer.mjs";
import { Buffer as read } from "node:buffer";
console.log(written === read, read.realmMarker.answer);"#,
        )
        .expect("write callable-property consumer");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build realm callable-property graph");
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping realm callable-property integration: no C compiler found");
            return;
        };
        let report = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain)
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native realm callable-property graph builds and runs");
        assert_eq!(
            report.exit_code,
            Some(0),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&report.stdout),
            String::from_utf8_lossy(&report.stderr)
        );
        assert_eq!(report.stdout, b"true 42\n");
    }

    #[test]
    fn native_realm_singletons_are_shared_across_generated_modules() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let root = TestDirectory::new("kiln-native-realm-singletons");
        fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
            .expect("write package metadata");
        fs::write(
            root.path.join("writer.mjs"),
            r#"globalThis.realmMarker = 41;
process.realmMarker = 42;
Atomics.realmMarker = 43;
export const ready = true;"#,
        )
        .expect("write singleton producer");
        let entry = root.path.join("main.mjs");
        fs::write(
            &entry,
            r#"import { ready } from "./writer.mjs";
console.log(ready, globalThis.realmMarker, process.realmMarker, Atomics.realmMarker);"#,
        )
        .expect("write singleton consumer");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build realm-singleton graph");
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping realm-singleton integration: no C compiler found");
            return;
        };
        let report = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain)
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native realm-singleton graph builds and runs");
        assert_eq!(
            report.exit_code,
            Some(0),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&report.stdout),
            String::from_utf8_lossy(&report.stderr)
        );
        assert_eq!(report.stdout, b"true 41 42 43\n");
    }

    #[test]
    fn native_commonjs_real_package_callable_executes_after_require() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root");
        let package_root = workspace.join("conformance/server/packages");
        // The probe imports a real package off disk, so it needs the conformance server's
        // dependencies installed. A fresh clone has no node_modules there and the resolver
        // raises a bare "cannot resolve" that reads like an engine bug; say what is missing.
        if !package_root.join("node_modules/safe-regex2").is_dir() {
            eprintln!(
                "skipping real CommonJS package integration: {} is missing. \
                 Run `npm ci` in {} to enable it.",
                package_root.join("node_modules/safe-regex2").display(),
                package_root.display()
            );
            return;
        }
        let entry = package_root.join(format!(
            ".kiln-native-safe-regex2-{}.mjs",
            std::process::id()
        ));
        fs::write(
            &entry,
            "import safe from 'safe-regex2'; console.log(safe(/a+/));",
        )
        .expect("write package probe");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&package_root).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build real package graph");
        let result = (|| {
            let toolchain = Toolchain::discover().ok()?;
            let cache = std::env::temp_dir().join(format!(
                "kiln-native-safe-regex-cache-{}",
                std::process::id()
            ));
            let builder = NativeBuilder::new(ArtifactCache::new(cache), toolchain);
            Some(
                builder.run_graph(
                    &graph,
                    &NativeBuildOptions {
                        output: std::env::temp_dir()
                            .join(format!("kiln-native-safe-regex-{}", std::process::id())),
                        profile: NativeProfile::Dev,
                        realm: NativeRealmPolicy::default(),
                    },
                ),
            )
        })();
        fs::remove_file(&entry).expect("remove package probe");
        let Some(report) = result else {
            eprintln!("skipping real CommonJS package integration: no C compiler found");
            return;
        };
        let report = report.expect("real CommonJS package runs natively");
        assert_eq!(report.stdout, b"true\n");
    }

    #[test]
    #[ignore = "diagnostic probe for the native Fastify constructor"]
    fn native_fastify_constructor_probe() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root");
        let package_root = workspace.join("conformance/server/packages");
        if !package_root
            .join("node_modules/fastify/fastify.js")
            .is_file()
        {
            return;
        }
        let entry = package_root.join(format!(
            ".kiln-native-fastify-constructor-{}.mjs",
            std::process::id()
        ));
        fs::write(
            &entry,
            "import Fastify from 'fastify'; const app = Fastify(); console.log(typeof app.get);",
        )
        .expect("write Fastify constructor probe");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&package_root).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build Fastify constructor graph");
        let result = Toolchain::discover().ok().map(|toolchain| {
            NativeBuilder::new(
                ArtifactCache::new(std::env::temp_dir().join("kiln-native-fastify-probe-cache")),
                toolchain,
            )
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: std::env::temp_dir().join("kiln-native-fastify-probe"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
        });
        fs::remove_file(&entry).expect("remove Fastify constructor probe");
        let Some(report) = result else { return };
        let report = report.expect("Fastify constructor runs natively");
        assert_eq!(
            report.exit_code,
            Some(0),
            "stderr={}",
            String::from_utf8_lossy(&report.stderr)
        );
        assert_eq!(report.stdout, b"function\n");
    }

    #[test]
    fn native_commonjs_builtin_namespace_is_not_its_default_export() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let root = TestDirectory::new("kiln-native-commonjs-builtin-namespace");
        fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
            .expect("write root package");
        fs::create_dir(root.path.join("pkg")).expect("create package directory");
        fs::write(root.path.join("pkg/package.json"), r#"{"type":"commonjs"}"#)
            .expect("write CommonJS package");
        fs::write(
            root.path.join("pkg/index.js"),
            "const buffer = require('node:buffer'); module.exports = typeof buffer.constants.MAX_STRING_LENGTH === 'number' && buffer.Buffer === buffer.default;",
        )
        .expect("write namespace consumer");
        let entry = root.path.join("main.mjs");
        fs::write(
            &entry,
            "import answer from './pkg/index.js'; console.log(answer);",
        )
        .expect("write entry");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build CommonJS namespace graph");

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native builtin namespace integration: no C compiler found");
            return;
        };
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        let run = builder
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("standalone graph runs");
        assert_eq!(run.stdout, b"true\n");
    }

    #[test]
    fn native_commonjs_require_materializes_captured_json_once() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let root = TestDirectory::new("kiln-native-commonjs-json");
        fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
            .expect("write root package");
        fs::create_dir(root.path.join("pkg")).expect("create package directory");
        fs::write(
            root.path.join("pkg/package.json"),
            r#"{"type":"commonjs","version":"7.4.2"}"#,
        )
        .expect("write CommonJS package JSON");
        fs::write(
            root.path.join("pkg/index.js"),
            "const first = require('./package.json'); const { version } = first; const second = require('./package.json'); console.log(first === second, version); module.exports = version;",
        )
        .expect("write CommonJS JSON consumer");
        let entry = root.path.join("main.mjs");
        fs::write(
            &entry,
            "import version from './pkg/index.js'; console.log(version);",
        )
        .expect("write entry");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build CommonJS JSON graph");
        assert!(graph.modules().any(|node| {
            node.source == ModuleSource::Json
                && node
                    .data
                    .as_deref()
                    .is_some_and(|text| text.contains("7.4.2"))
        }));
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native CommonJS JSON execution: no C compiler found");
            return;
        };
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        let report = builder
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("native CommonJS JSON graph builds and runs");
        assert_eq!(report.stdout, b"true 7.4.2\n7.4.2\n");
    }

    #[test]
    fn native_graphs_materialize_one_snapshot_namespace_per_provider() {
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

        let launcher = graph_launcher_source(&graph, &modules).expect("launcher emits");
        assert!(launcher.contains("kiln_imports_1[0] = kiln_namespaces[0];"));
        assert_eq!(launcher.matches("kiln_namespaces[0]").count(), 2);
        assert!(launcher.contains("_namespace_wrapper(&kiln_namespaces[0]"));
        let wrapper = graph_shared_wrapper_source(&graph, &modules, "kiln_namespace_graph")
            .expect("shared wrapper emits");
        assert!(wrapper.contains("import_arrays[1][0] = namespaces[0];"));
        assert!(wrapper.contains("_namespace_wrapper(&namespaces[0]"));

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native namespace integration: no C compiler found");
            return;
        };
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        let report = builder
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("namespace graph builds and runs");
        assert_eq!(report.stdout, b"42\n");

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let shared = builder
                .build_shared_graph(
                    &graph,
                    &NativeBuildOptions {
                        output: root.path.join("namespace").with_extension(
                            if cfg!(target_os = "macos") {
                                "dylib"
                            } else {
                                "so"
                            },
                        ),
                        profile: NativeProfile::Dev,
                        realm: NativeRealmPolicy::default(),
                    },
                )
                .expect("namespace shared graph links");
            assert!(shared.output.is_file());
        }
    }

    #[test]
    fn native_namespace_reexport_reuses_the_provider_namespace_identity() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native namespace re-export integration: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-namespace-reexport");
        let origin = root.path.join("origin.mjs");
        let barrel = root.path.join("barrel.mjs");
        let entry = root.path.join("main.mjs");
        fs::write(&origin, "export const answer = 42;\n").expect("write namespace origin");
        fs::write(&barrel, "export * as origin from './origin.mjs';\n")
            .expect("write namespace barrel");
        fs::write(
            &entry,
            r#"import * as direct from "./origin.mjs";
import { origin as via } from "./barrel.mjs";
console.log(direct === via, via.answer);
"#,
        )
        .expect("write namespace consumer");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("namespace re-export graph compiles");
        let report = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain)
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("namespace re-export graph builds and runs");
        assert_eq!(report.stdout, b"true 42\n");
    }

    #[test]
    fn native_namespace_refuses_reserved_string_exports_before_emission() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let root = TestDirectory::new("kiln-native-namespace-reserved-export");
        let origin = root.path.join("origin.mjs");
        let entry = root.path.join("main.mjs");
        fs::write(
            &origin,
            "const value = 1; export { value as \"@@toStringTag\" };\n",
        )
        .expect("write reserved namespace export");
        fs::write(
            &entry,
            "import * as namespace from './origin.mjs'; console.log(namespace);\n",
        )
        .expect("write namespace consumer");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("reserved export graph compiles");
        let modules = graph
            .modules()
            .map(|node| {
                (
                    node,
                    kiln_backend_c::emit_module(&node.program).expect("module emits"),
                )
            })
            .collect::<Vec<_>>();
        let error = graph_launcher_source(&graph, &modules)
            .expect_err("reserved namespace export must refuse before C emission");
        assert!(matches!(
            error,
            NativeError::Emit(EmitError::Unsupported { feature })
                if feature == "@@-prefixed exports in native module namespaces"
        ));
    }

    #[test]
    fn native_named_reexport_of_commonjs_default_keeps_slot_zero() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping CommonJS re-export integration: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-commonjs-named-reexport");
        fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
            .expect("write root package");
        fs::create_dir(root.path.join("pkg")).expect("create CommonJS package");
        fs::write(root.path.join("pkg/package.json"), r#"{"type":"commonjs"}"#)
            .expect("write CommonJS package metadata");
        fs::write(root.path.join("pkg/index.js"), "module.exports = 42;\n")
            .expect("write CommonJS provider");
        fs::write(
            root.path.join("barrel.mjs"),
            "export { default as answer } from './pkg/index.js';\n",
        )
        .expect("write ESM barrel");
        let entry = root.path.join("main.mjs");
        fs::write(
            &entry,
            "import { answer } from './barrel.mjs'; console.log(answer);\n",
        )
        .expect("write ESM consumer");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("CommonJS re-export graph compiles");
        let report = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain)
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("program"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("CommonJS re-export graph builds and runs");
        assert_eq!(report.stdout, b"42\n");
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
            source: "".into(),
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
                    realm: NativeRealmPolicy::default(),
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
                    realm: NativeRealmPolicy::default(),
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
            source: "".into(),
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
                    realm: NativeRealmPolicy::default(),
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
            realm: NativeRealmPolicy::default(),
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
                    realm: NativeRealmPolicy::default(),
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
        let package = shared_publication::read_shared_library_package(&second.output)
            .expect("read scalar shared-library package");
        assert_eq!(package.manifest().legacy_entry_symbol, second.module_symbol);
        assert_eq!(package.manifest().v2_api_symbol, second.module_api_symbol);
        assert_eq!(
            fs::read(package.header_path()).unwrap(),
            fs::read(&second.header).unwrap()
        );
        assert_eq!(
            fs::read(package.loader_path()).unwrap(),
            fs::read(&second.loader).unwrap()
        );
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
                    realm: NativeRealmPolicy::default(),
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
    *result = (kiln_value){ KILN_VALUE_UNDEFINED, 0, {0} };
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
    fn owned_result_scalar_api_copies_strings_and_is_idempotent() {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            eprintln!("skipping owned-result native integration on unsupported platform");
            return;
        }
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping owned-result native integration: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-owned-result-test");
        let builder = NativeBuilder::new(
            ArtifactCache::new(root.path.join("cache")),
            toolchain.clone(),
        );
        let program = Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::String("left\0right".to_owned())],
            code: vec![Instruction::Const(0), Instruction::Return],
            local_count: 0,
            functions: Vec::new(),
        };
        let report = builder
            .build_shared_library(
                &program,
                &NativeBuildOptions {
                    output: root.path.join(shared_library_name()),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("build owned-result scalar library");
        assert_eq!(
            report.module_api_symbol,
            format!("{}_api_v2", report.module_symbol)
        );
        let header_name = report
            .header
            .file_name()
            .and_then(|name| name.to_str())
            .expect("owned-result header file name");
        let host_source = root.path.join("host.c");
        let host_object = root.path.join("host.o");
        let host_executable = root.path.join("host");
        fs::write(
            &host_source,
            format!(
                r#"#include "{header_name}"
#include <string.h>

int main(void) {{
    const kiln_module_api_v2 *api = {api_symbol}();
    kiln_result_v2 *owned = NULL;
    kiln_value value;
    kiln_value legacy;
    int index;
    if (api == NULL || api->abi_version != KILN_MODULE_API_VERSION ||
        api->struct_size < sizeof(*api) || api->run == NULL || api->read == NULL ||
        api->dispose == NULL) return 1;
    if ({module_symbol}(NULL, &legacy) != KILN_C99_OK ||
        legacy.tag != KILN_VALUE_STRING || legacy.len != 10 ||
        memcmp(legacy.ptr, "left\0right", 10) != 0) return 2;
    for (index = 0; index < 64; ++index) {{
        if (api->run(NULL, &owned) != KILN_C99_OK || owned == NULL) return 3;
        if (api->read(owned, &value) != KILN_C99_OK || value.tag != KILN_VALUE_STRING ||
            value.len != 10 || value.ptr == NULL || memcmp(value.ptr, "left\0right", 10) != 0)
            return 4;
        api->dispose(&owned);
        if (owned != NULL) return 5;
        api->dispose(&owned);
    }}
    return 0;
}}
"#,
                api_symbol = report.module_api_symbol,
                module_symbol = report.module_symbol,
            ),
        )
        .expect("write owned-result host");
        let mut compile_options = CompileOptions::default();
        compile_options.include_dirs.push(root.path.clone());
        toolchain
            .compile_c(&host_source, &host_object, &compile_options)
            .expect("compile owned-result host");
        let mut link_options = LinkOptions::default();
        link_options.runtime_libraries.push(report.output.clone());
        let loader_directory = report.loader.parent().expect("loader has directory");
        link_options
            .extra_args
            .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
        toolchain
            .link_executable(&[host_object], &host_executable, &link_options)
            .expect("link owned-result host");
        let mut command = Command::new(&host_executable);
        #[cfg(target_os = "linux")]
        command.env("LD_LIBRARY_PATH", loader_directory);
        #[cfg(target_os = "macos")]
        command.env("DYLD_LIBRARY_PATH", loader_directory);
        let output = command.output().expect("run owned-result host");
        assert!(
            output.status.success(),
            "owned-result host failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        #[cfg(target_os = "linux")]
        {
            let symbols = Command::new("nm")
                .args(["-D", "--defined-only"])
                .arg(&report.loader)
                .output()
                .expect("nm is available");
            assert!(symbols.status.success());
            let mut exported = String::from_utf8_lossy(&symbols.stdout)
                .lines()
                .filter_map(|line| line.split_whitespace().last())
                .filter(|symbol| symbol.starts_with("kiln_"))
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            exported.sort_unstable();
            let mut expected = vec![report.module_symbol, report.module_api_symbol];
            expected.sort_unstable();
            assert_eq!(exported, expected);
        }
    }

    #[test]
    fn owned_result_scalar_api_rejects_malformed_host_strings() {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            eprintln!(
                "skipping owned-result scalar malformed-host integration on unsupported platform"
            );
            return;
        }
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!(
                "skipping owned-result scalar malformed-host integration: no C compiler found"
            );
            return;
        };
        let root = TestDirectory::new("kiln-native-owned-result-malformed-scalar-test");
        let program = Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 0,
                },
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };
        let report = NativeBuilder::new(
            ArtifactCache::new(root.path.join("cache")),
            toolchain.clone(),
        )
        .build_shared_library(
            &program,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
            },
        )
        .expect("build malformed-host scalar library");
        let header_name = report
            .header
            .file_name()
            .and_then(|name| name.to_str())
            .expect("malformed-host scalar header file name");
        let host_source = root.path.join("host.c");
        fs::write(
            &host_source,
            format!(
                r#"#include "{header_name}"

static int malformed_call(void *context, uint16_t operation, uint16_t argc,
                          const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    (void)operation;
    (void)argc;
    (void)arguments;
    if (result == NULL) return KILN_C99_HOST_ERROR;
    *result = (kiln_value){{ KILN_VALUE_STRING, 3, {{0}} }};
    return KILN_C99_OK;
}}

int main(void) {{
    kiln_host host = {{malformed_call, NULL}};
    const kiln_module_api_v2 *api = {api_symbol}();
    kiln_result_v2 *owned = NULL;
    if (api == NULL) return 1;
    if (api->run(&host, &owned) != KILN_C99_INVALID_ARGUMENT || owned != NULL)
        return 2;
    api->dispose(&owned);
    return 0;
}}
"#,
                api_symbol = report.module_api_symbol,
            ),
        )
        .expect("write malformed-host scalar host");
        let host_object = root.path.join("host.o");
        let mut compile_options = CompileOptions::default();
        compile_options.include_dirs.push(root.path.clone());
        toolchain
            .compile_c(&host_source, &host_object, &compile_options)
            .expect("compile malformed-host scalar host");
        let host_executable = root.path.join("host");
        let mut link_options = LinkOptions::default();
        link_options.runtime_libraries.push(report.output.clone());
        let loader_directory = report.loader.parent().expect("loader has directory");
        link_options
            .extra_args
            .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
        toolchain
            .link_executable(&[host_object], &host_executable, &link_options)
            .expect("link malformed-host scalar host");
        let mut command = Command::new(&host_executable);
        #[cfg(target_os = "linux")]
        command.env("LD_LIBRARY_PATH", loader_directory);
        #[cfg(target_os = "macos")]
        command.env("DYLD_LIBRARY_PATH", loader_directory);
        let output = command.output().expect("run malformed-host scalar host");
        assert!(
            output.status.success(),
            "malformed-host scalar failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn owned_result_graph_api_preserves_uncaught_thrown_scalars() {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            eprintln!("skipping owned-result graph throw integration on unsupported platform");
            return;
        }
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping owned-result graph throw integration: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-owned-result-throw-test");
        let project = root.path.join("project");
        fs::create_dir_all(&project).expect("create throw graph project");
        fs::write(project.join("main.js"), "").expect("write throw graph entry");
        let thrown = ModuleProgram {
            program: Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: vec![Constant::String("thrown\0value".to_owned())],
                code: vec![Instruction::Const(0), Instruction::Throw],
                local_count: 0,
                functions: Vec::new(),
            },
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            strictness: kiln_ir::Strictness::default(),
        };
        let graph = ModuleGraph::build_with_modules(project.join("main.js"), &project, move |_| {
            thrown.clone()
        })
        .expect("construct throw graph");
        let report = NativeBuilder::new(
            ArtifactCache::new(root.path.join("cache")),
            toolchain.clone(),
        )
        .build_shared_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
            },
        )
        .expect("build throw graph");
        let header_name = report
            .header
            .file_name()
            .and_then(|name| name.to_str())
            .expect("throw graph header file name");
        let host_source = root.path.join("host.c");
        let host_object = root.path.join("host.o");
        let host_executable = root.path.join("host");
        fs::write(
            &host_source,
            format!(
                r#"#include "{header_name}"
#include <string.h>

int main(void) {{
    const kiln_module_api_v2 *api = {api_symbol}();
    kiln_result_v2 *owned = NULL;
    kiln_value value;
    int status;
    if (api == NULL) return 1;
    status = api->run(NULL, &owned);
    if (status != KILN_C99_THROWN || owned == NULL) return 2;
    if (api->read(owned, &value) != KILN_C99_OK || value.tag != KILN_VALUE_STRING ||
        value.len < 12 || value.ptr == NULL || memcmp(value.ptr, "thrown\0value", 12) != 0)
        return 3;
    api->dispose(&owned);
    if (owned != NULL) return 4;
    api->dispose(&owned);
    return 0;
}}
"#,
                api_symbol = report.module_api_symbol,
            ),
        )
        .expect("write throw graph host");
        let mut compile_options = CompileOptions::default();
        compile_options.include_dirs.push(root.path.clone());
        toolchain
            .compile_c(&host_source, &host_object, &compile_options)
            .expect("compile throw graph host");
        let mut link_options = LinkOptions::default();
        link_options.runtime_libraries.push(report.output.clone());
        let loader_directory = report.loader.parent().expect("loader has directory");
        link_options
            .extra_args
            .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
        toolchain
            .link_executable(&[host_object], &host_executable, &link_options)
            .expect("link throw graph host");
        let mut command = Command::new(&host_executable);
        #[cfg(target_os = "linux")]
        command.env("LD_LIBRARY_PATH", loader_directory);
        #[cfg(target_os = "macos")]
        command.env("DYLD_LIBRARY_PATH", loader_directory);
        let output = command.output().expect("run throw graph host");
        assert!(
            output.status.success(),
            "throw graph host failed with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
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
                        realm: NativeRealmPolicy::default(),
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
                            realm: NativeRealmPolicy::default(),
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
                kiln_interpreter::ExecutionCapabilities::default(),
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
                        realm: NativeRealmPolicy::default(),
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
    fn native_json_errors_are_catchable_and_uncaught_errors_are_reported() {
        use kiln_module_resolver::ModuleResolver;

        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native JSON exception integration: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-json-exceptions");
        let project = root.path.join("project");
        fs::create_dir_all(&project).expect("create JSON exception project");
        let entry = project.join("main.js");
        fs::write(
            &entry,
            "try { JSON.parse('{oops'); } catch (error) { console.log('parse', error instanceof SyntaxError, error.constructor.name); }\nconsole.log('after-parse');\nconst cycle = {}; cycle.self = cycle; try { JSON.stringify(cycle); } catch (error) { console.log('stringify', error instanceof TypeError, error.constructor.name); }\nconsole.log('after-stringify');\n",
        )
        .expect("write catchable JSON exception source");
        let mut graph = ModuleGraph::build_node(
            &entry,
            ModuleResolver::new(&project).expect("module resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("catchable JSON exception source compiles");
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        let caught = builder
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("caught"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("catchable JSON exceptions run natively");
        assert_eq!(caught.exit_code, Some(0));
        assert_eq!(
            caught.stdout,
            b"parse true SyntaxError\nafter-parse\nstringify true TypeError\nafter-stringify\n"
        );
        assert!(caught.stderr.is_empty());

        fs::write(&entry, "JSON.parse('{oops');\n").expect("write uncaught JSON exception source");
        graph = ModuleGraph::build_node(
            &entry,
            ModuleResolver::new(&project).expect("module resolver for uncaught source"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("uncaught JSON exception source compiles");
        let uncaught = builder
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("uncaught"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("uncaught JSON exception run reports a native result");
        assert_eq!(uncaught.exit_code, Some(1));
        assert!(uncaught.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&uncaught.stderr)
                .contains("uncaught SyntaxError: Unexpected token in JSON")
        );

        fs::write(
            &entry,
            "const cycle = {}; cycle.self = cycle; JSON.stringify(cycle);\n",
        )
        .expect("write uncaught cyclic JSON source");
        graph = ModuleGraph::build_node(
            &entry,
            ModuleResolver::new(&project).expect("module resolver for cyclic source"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("uncaught cyclic JSON source compiles");
        let uncaught_cycle = builder
            .run_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("uncaught-cycle"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("uncaught cyclic JSON run reports a native result");
        assert_eq!(uncaught_cycle.exit_code, Some(1));
        assert!(uncaught_cycle.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&uncaught_cycle.stderr)
                .contains("uncaught TypeError: Converting circular structure to JSON")
        );
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
                source: "".into(),
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
            strictness: kiln_ir::Strictness::default(),
        };
        let entry = ModuleProgram {
            program: Program {
                source: "".into(),
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
            strictness: kiln_ir::Strictness::default(),
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
        let main = launcher
            .find("int main(int argc, char **argv)")
            .expect("launcher has main");
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
                    realm: NativeRealmPolicy::default(),
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
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("second graph build succeeds");
        assert!(second.build.cache_hits.generated_c);
        assert!(second.build.cache_hits.application_object);
        // Diagnostics use logical project-relative paths, so sibling roots reuse launchers too.
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
                source: "".into(),
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
            strictness: kiln_ir::Strictness::default(),
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
                    realm: NativeRealmPolicy::default(),
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
    fn commonjs_shared_identity_includes_observable_absolute_path() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let root = TestDirectory::new("kiln-native-commonjs-shared-path-identity");
        let first = root.path.join("first");
        let second = root.path.join("second");
        for project in [&first, &second] {
            fs::create_dir_all(project).expect("create CommonJS project");
            fs::write(project.join("package.json"), r#"{"type":"commonjs"}"#)
                .expect("write CommonJS package metadata");
            fs::write(project.join("main.js"), "module.exports = __filename;")
                .expect("write CommonJS entry");
        }

        let identity = |project: &Path| {
            let entry = project.join("main.js");
            let graph = ModuleGraph::build_node(
                &entry,
                NodeRuntimeResolver::new(project).expect("node resolver"),
                |path: &Path| fs::read_to_string(path),
                OxcFrontend,
            )
            .expect("build CommonJS graph");
            let modules = graph
                .modules()
                .map(|node| {
                    let logical = logical_module_path(graph.project_root(), &node.path)
                        .expect("logical module path");
                    (
                        node,
                        emit_native_graph_module(node, &logical, &NativeRealmPolicy::default())
                            .expect("module emits"),
                    )
                })
                .collect::<Vec<_>>();
            let topology = graph_shared_topology_digest(&graph, &modules);
            let public = graph_shared_public_digest(topology, &modules)
                .expect("CommonJS graph has absolute module paths");
            let module_sources = modules
                .iter()
                .map(|(_, module)| Digest::hash(module.source.as_bytes()))
                .collect::<Vec<_>>();
            let symbol = format!("kiln_shared_graph_{}", public.to_hex());
            let wrapper = graph_shared_wrapper_source(&graph, &modules, &symbol)
                .expect("shared wrapper emits");
            (topology, public, module_sources, wrapper)
        };

        let first_identity = identity(&first);
        let second_identity = identity(&second);
        assert_eq!(first_identity.0, second_identity.0);
        assert_eq!(first_identity.2, second_identity.2);
        assert_ne!(first_identity.1, second_identity.1);
        assert!(
            first_identity.3.contains(&native_c_string(
                &fs::canonicalize(first.join("main.js"))
                    .expect("canonical first entry")
                    .to_string_lossy()
            ))
        );
        assert!(
            second_identity.3.contains(&native_c_string(
                &fs::canonicalize(second.join("main.js"))
                    .expect("canonical second entry")
                    .to_string_lossy()
            ))
        );
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
                source: "".into(),
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
            strictness: kiln_ir::Strictness::default(),
        };
        let entry = ModuleProgram {
            program: Program {
                source: "".into(),
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
            strictness: kiln_ir::Strictness::default(),
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
        assert!(wrapper.contains(&format!("{}_api_v2", wrapper_symbol)));
        assert!(wrapper.contains("preserve_result"));
        assert!(wrapper.contains("kiln_module_api_v2"));
        assert!(wrapper.contains("static int kiln_v2_graph_"));
        assert!(!wrapper.contains("static kiln_result_v2"));
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
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("build shared graph succeeds");
        assert!(report.module_symbol.starts_with("kiln_shared_graph_"));
        assert!(report.output.is_file());
        let cached = builder
            .build_shared_graph(
                &graph,
                &NativeBuildOptions {
                    output: report.output.clone(),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
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
                    realm: NativeRealmPolicy::default(),
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
        let target = target_identity(toolchain.identity());
        let (provider, _, _) = builder
            .cached_number_provider(&target)
            .expect("load Realm provider");
        let provider_archive = root.path.join("realm-provider.a");
        materialize(&provider_archive, &provider, false).expect("materialize Realm provider");
        let library = root.path.join(shared_library_name());
        let mut manual_link = LinkOptions::default();
        manual_link.runtime_libraries.push(provider_archive);
        if cfg!(not(target_os = "windows")) {
            manual_link.libraries.push("m".to_owned());
        }
        toolchain
            .link_shared_library(&objects, &library, &manual_link)
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
                source: "".into(),
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
            strictness: kiln_ir::Strictness::default(),
        };
        let entry = ModuleProgram {
            program: Program {
                source: "".into(),
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
            strictness: kiln_ir::Strictness::default(),
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
                    realm: NativeRealmPolicy::default(),
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
            *result = (kiln_value){{ KILN_VALUE_STRING, 3, {{0}} }};
            return KILN_C99_OK;
        }}
        if (context->mode == 1) {{
            *result = (kiln_value){{ KILN_VALUE_STRING, 0, {{0}} }};
            return KILN_C99_OK;
        }}
        memcpy(context->storage, "dep", 3);
        *result = (kiln_value){{KILN_VALUE_STRING, 3, {{0}}}};
        result->ptr = context->storage;
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
    if (context->mode == 0) {{
        *result = (kiln_value){{KILN_VALUE_STRING, 3, {{0}}}};
        result->ptr = context->storage;
    }} else {{
        *result = (kiln_value){{ KILN_VALUE_UNDEFINED, 0, {{0}} }};
    }}
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
    {{
        const kiln_module_api_v2 *api = {api_symbol}();
        kiln_result_v2 *owned = NULL;
        if (api == NULL || api->abi_version != KILN_MODULE_API_VERSION) return 7;
        context.mode = 0;
        for (index = 0; index < 64; ++index) {{
            if (api->run(&host, &owned) != KILN_C99_OK || owned == NULL) return 8;
            if (api->read(owned, &result) != KILN_C99_OK || result.tag != KILN_VALUE_STRING ||
                result.len != 3 || result.ptr == NULL || memcmp(result.ptr, "dep", 3) != 0)
                return 9;
            api->dispose(&owned);
            if (owned != NULL) return 10;
            api->dispose(&owned);
        }}
        context.mode = 2;
        if (api->run(&host, &owned) != KILN_C99_HOST_ERROR || owned != NULL)
            return 11;
        context.mode = 3;
        if (api->run(&host, &owned) != KILN_C99_INVALID_ARGUMENT || owned != NULL)
            return 12;
    }}
    return 0;
}}
"#,
                symbol = report.module_symbol,
                api_symbol = report.module_api_symbol,
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
            source: "".into(),
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
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("build first same-basename library");
        let second = builder
            .build_shared_library(
                &program(22.0),
                &NativeBuildOptions {
                    output: second_dir.join(leaf),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
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
        let first_graph = one_module_graph(&root.path.join("first-project"), 11.0, "value");
        let second_graph = one_module_graph(&root.path.join("second-project"), 22.0, "value");
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
                    realm: NativeRealmPolicy::default(),
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
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("build second graph library");
        assert_ne!(
            first.module_symbol, second.module_symbol,
            "the host must be able to address both public wrappers independently"
        );
        assert_ne!(
            first.module_api_symbol, second.module_api_symbol,
            "owned-result accessors must not interpose across same-topology DSOs"
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
        // Zig's Mach-O linker does not yet implement `-exported_symbols_list`
        // (ziglang/zig#24662). The generated module entries are still hidden by object
        // visibility, which is the interposition property this regression protects; exercise
        // the stronger provider-export policy whenever the selected linker can enforce it.
        let provider_visibility_checks =
            if cfg!(target_os = "macos") && toolchain.identity().kind == CompilerKind::Zig {
                ""
            } else {
                r#"
    if (dlsym(first, "kiln_number_to_js_string") != NULL ||
        dlsym(second, "kiln_number_to_js_string") != NULL ||
        dlsym(first, "kiln_string_to_number") != NULL ||
        dlsym(second, "kiln_string_to_number") != NULL ||
        dlsym(first, "kiln_realm_create") != NULL ||
        dlsym(second, "kiln_realm_create") != NULL ||
        dlsym(first, "kiln_realm_drive") != NULL ||
        dlsym(second, "kiln_realm_drive") != NULL)
        return 4;
"#
            };
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
    void *symbol = dlsym(handle, name);
    kiln_entry_fn entry = NULL;
    if (symbol == NULL || sizeof(entry) != sizeof(symbol)) return NULL;
    memcpy(&entry, &symbol, sizeof(entry));
    return entry;
}}

int main(int argc, char **argv) {{
    kiln_value result;
    void *first;
    void *second;
    kiln_entry_fn first_entry;
    kiln_entry_fn second_entry;
    if (argc != 3) return 1;
    first = dlopen(argv[1], RTLD_NOW | RTLD_GLOBAL);
    if (first == NULL) return 2;
    second = dlopen(argv[2], RTLD_NOW | RTLD_LOCAL);
    if (second == NULL) return 3;
    if (dlsym(first, "{internal}") != NULL || dlsym(second, "{internal}") != NULL)
        return 4;
    {provider_visibility_checks}
    first_entry = load_entry(first, "{first_symbol}");
    second_entry = load_entry(second, "{second_symbol}");
    if (first_entry == NULL || second_entry == NULL) return 5;
    if (first_entry(NULL, &result) != KILN_C99_OK ||
        result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({first_bits}))
        return 1;
    if (second_entry(NULL, &result) != KILN_C99_OK ||
        result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({second_bits}))
        return 2;
    {{
        const kiln_module_api_v2 *first_api = {first_api_symbol}();
        const kiln_module_api_v2 *second_api = {second_api_symbol}();
        kiln_result_v2 *first_owned = NULL;
        kiln_result_v2 *second_owned = NULL;
        if (first_api == NULL || second_api == NULL ||
            first_api->run(NULL, &first_owned) != KILN_C99_OK || first_owned == NULL ||
            second_api->run(NULL, &second_owned) != KILN_C99_OK || second_owned == NULL)
            return 3;
        if (first_api->read(first_owned, &result) != KILN_C99_OK ||
            result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({first_bits})) return 4;
        if (second_api->read(second_owned, &result) != KILN_C99_OK ||
            result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({second_bits})) return 5;
        first_api->dispose(&first_owned);
        second_api->dispose(&second_owned);
        if (first_owned != NULL || second_owned != NULL) return 6;
        first_api->dispose(&first_owned);
        second_api->dispose(&second_owned);
    }}
    dlclose(second);
    dlclose(first);
    return 0;
}}
"#,
                first_symbol = first.module_symbol,
                second_symbol = second.module_symbol,
                first_api_symbol = first.module_api_symbol,
                second_api_symbol = second.module_api_symbol,
                internal = first_internal,
                first_bits = 11.0_f64.to_bits(),
                second_bits = 22.0_f64.to_bits(),
                provider_visibility_checks = provider_visibility_checks,
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
        let mut link_options = LinkOptions {
            runtime_libraries: vec![first.output.clone(), second.output.clone()],
            libraries: if cfg!(target_os = "linux") {
                vec!["dl".to_owned()]
            } else {
                Vec::new()
            },
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
            String::from_utf8_lossy(&output.stderr),
        );
    }

    fn stdout_program(text: &str, result: f64) -> Program {
        Program {
            source: "".into(),
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

    #[test]
    fn native_host_clock_is_explicit_while_default_date_stays_at_epoch() {
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native host-clock integration: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-host-clock");
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        let program = Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::String("now".to_owned())],
            code: vec![
                Instruction::LoadBuiltin(kiln_ir::Builtin::Date),
                Instruction::CallMethod { name: 0, argc: 0 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };

        let deterministic = builder
            .run(
                &program,
                &NativeBuildOptions {
                    output: root.path.join("deterministic"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                },
            )
            .expect("deterministic Date program runs");
        assert_eq!(deterministic.result, Some(HostValue::number(0.0)));

        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock is after epoch")
            .as_secs_f64()
            * 1_000.0;
        let host_backed = builder
            .run(
                &program,
                &NativeBuildOptions {
                    output: root.path.join("host"),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::host_clock(),
                },
            )
            .expect("host-backed Date program runs");
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock is after epoch")
            .as_secs_f64()
            * 1_000.0;
        let observed = host_backed
            .result
            .as_ref()
            .and_then(HostValue::as_number)
            .expect("Date.now returns a number");
        assert!(
            observed >= before && observed <= after,
            "host Date.now {observed} outside build/run interval {before}..={after}"
        );
        assert_ne!(
            deterministic.build.module_symbol,
            host_backed.build.module_symbol
        );
    }

    #[test]
    fn native_entropy_policy_is_reproducible_or_explicitly_host_backed() {
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native entropy integration: no C compiler found");
            return;
        };
        let root = TestDirectory::new("kiln-native-entropy");
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        let program = Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::String("random".to_owned())],
            code: vec![
                Instruction::LoadBuiltin(kiln_ir::Builtin::Math),
                Instruction::CallMethod { name: 0, argc: 0 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };
        let run = |name: &str, entropy: NativeEntropyPolicy| {
            builder
                .run(
                    &program,
                    &NativeBuildOptions::new(root.path.join(name), NativeProfile::Dev)
                        .with_realm_policy(NativeRealmPolicy::default().with_entropy(entropy)),
                )
                .unwrap_or_else(|error| panic!("native entropy fixture {name} failed: {error}"))
        };

        let deterministic = run("deterministic", NativeEntropyPolicy::Deterministic);
        let seeded_a = run(
            "seeded-a",
            NativeEntropyPolicy::Seeded(kiln_runtime::DEFAULT_ENTROPY_SEED),
        );
        let seeded_b = run(
            "seeded-b",
            NativeEntropyPolicy::Seeded(kiln_runtime::DEFAULT_ENTROPY_SEED),
        );
        let other_seed = run("seeded-other", NativeEntropyPolicy::Seeded(7));
        assert_eq!(deterministic.result, seeded_a.result);
        assert_eq!(seeded_a.result, seeded_b.result);
        assert_eq!(seeded_a.build.module_symbol, seeded_b.build.module_symbol);
        assert_ne!(seeded_a.build.module_symbol, other_seed.build.module_symbol);
        assert_ne!(seeded_a.result, other_seed.result);

        let host_a = run("host-a", NativeEntropyPolicy::Host);
        let host_b = run("host-b", NativeEntropyPolicy::Host);
        for report in [&host_a, &host_b] {
            let value = report
                .result
                .as_ref()
                .and_then(HostValue::as_number)
                .expect("Math.random returns a number");
            assert!((0.0..1.0).contains(&value));
        }
        assert_eq!(host_a.build.module_symbol, host_b.build.module_symbol);
        assert_ne!(
            host_a.build.module_symbol,
            deterministic.build.module_symbol
        );
    }

    #[test]
    fn native_deterministic_entropy_is_one_stream_across_modules() {
        use kiln_module_resolver::NodeRuntimeResolver;

        let root = TestDirectory::new("kiln-native-graph-entropy");
        fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
            .expect("write package metadata");
        fs::write(
            root.path.join("first.mjs"),
            "export const first = Math.random();",
        )
        .expect("write first entropy consumer");
        let entry = root.path.join("main.mjs");
        fs::write(
            &entry,
            r#"import { first } from "./first.mjs";
console.log(first === Math.random());"#,
        )
        .expect("write second entropy consumer");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&root.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build entropy graph");
        let Some(toolchain) = Toolchain::discover().ok() else {
            eprintln!("skipping native graph entropy integration: no C compiler found");
            return;
        };
        let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
        for (name, entropy) in [
            ("default", NativeEntropyPolicy::Deterministic),
            ("seeded", NativeEntropyPolicy::Seeded(7)),
        ] {
            let report = builder
                .run_graph(
                    &graph,
                    &NativeBuildOptions::new(root.path.join(name), NativeProfile::Dev)
                        .with_realm_policy(NativeRealmPolicy::default().with_entropy(entropy)),
                )
                .unwrap_or_else(|error| panic!("native graph entropy fixture failed: {error}"));
            assert_eq!(
                report.exit_code,
                Some(0),
                "stderr={}",
                String::from_utf8_lossy(&report.stderr)
            );
            assert_eq!(report.stdout, b"false\n");
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
            source: "".into(),
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
            source: "".into(),
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
                    source: "".into(),
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
                    source: "".into(),
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
                    source: "".into(),
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
                    source: "".into(),
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
                    source: "".into(),
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

    /// Every native builtin body must fill exactly the slots the shared export table declares.
    ///
    /// The bodies index `exports[]` by position, and the positions are decided in the leaf ABI
    /// crate. Nothing makes that a compile error, and it has already
    /// broken once: `node:http`'s body was written against a six-name list, the table later grew
    /// three names at the front, and every slot shifted by two without a single warning. The
    /// result was not a refusal but a wrong answer - `default` bound to `request`, `createServer`
    /// bound to `get` - which is the failure mode this repository treats as the expensive one.
    ///
    /// So the coupling is pinned here rather than trusted. Adding a name to the shared table now
    /// fails this test, which is the cheapest place to find out.
    #[test]
    fn native_builtin_slots_match_the_shared_export_table() {
        use kiln_module_resolver::NodeBuiltin;

        for builtin in NodeBuiltin::ALL.iter().copied() {
            let Some(body) = native_builtin_body(builtin) else {
                // Resolver-known but not-yet-materialized builtins are deliberately rejected by
                // `validate_native_node_builtins`; they have no native slot contract to check.
                continue;
            };
            let specifier = builtin.specifier();
            let abi = kiln_node_abi::builtin_abi(specifier)
                .unwrap_or_else(|| panic!("{specifier} has no declared Node ABI row"));
            for supported in native_builtin_supported_exports(builtin) {
                assert!(
                    abi.exports.contains(supported),
                    "{specifier} claims native export `{supported}` outside its declared ABI {:?}",
                    abi.exports
                );
            }

            let mut assigned: Vec<usize> = body
                .lines()
                .filter_map(|line| line.trim().strip_prefix("exports["))
                .filter_map(|rest| rest.split(']').next())
                .map(|index| {
                    index
                        .parse()
                        .unwrap_or_else(|_| panic!("{specifier}: unparseable slot `{index}`"))
                })
                .collect();
            assigned.sort_unstable();

            let expected: Vec<usize> = (0..abi.exports.len()).collect();
            assert_eq!(
                assigned, expected,
                "{specifier} assigns slots {assigned:?} but its declared exports are {:?}; \
                 a duplicate slot is also invalid; \
                 a body that skips a slot leaves an export `undefined` and one that overruns \
                 writes past the array",
                abi.exports
            );
        }
    }

    #[test]
    fn native_builtin_support_is_the_materialized_subset_not_the_declared_surface() {
        use kiln_module_resolver::NodeBuiltin;

        assert!(native_builtin_supports_export(
            NodeBuiltin::Fs,
            "existsSync"
        ));
        assert!(!native_builtin_supports_export(
            NodeBuiltin::Fs,
            "accessSync"
        ));
        assert!(native_builtin_supports_export(NodeBuiltin::Path, "join"));
        assert!(!native_builtin_supports_export(
            NodeBuiltin::Path,
            "basename"
        ));
        assert!(native_builtin_supports_export(NodeBuiltin::Os, "hostname"));
        assert!(!native_builtin_supports_export(NodeBuiltin::Os, "platform"));
        assert!(!native_builtin_supports_export(
            NodeBuiltin::Http,
            "IncomingMessage"
        ));
    }

    #[test]
    fn resolver_and_node_abi_cover_the_same_builtin_set() {
        let resolver = kiln_module_resolver::NodeBuiltin::ALL
            .iter()
            .map(|builtin| builtin.specifier())
            .collect::<BTreeSet<_>>();
        let abi = kiln_node_abi::SPECIFIERS
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        assert_eq!(resolver, abi);
    }
}
