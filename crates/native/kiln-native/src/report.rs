use super::*;
use kiln_artifact_cache::CacheError;
use kiln_backend_c::EmitError;
use kiln_host_abi::HostValue;
use kiln_module_graph::ModuleGraphError;
use kiln_native_runtime::RuntimeSourceError;
use kiln_native_toolchain::ToolchainError;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

/// Which artifact stages were reused without invoking the platform compiler or linker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheHits {
    /// For graph builds, true only when every module's generated C artifact hit.
    pub generated_c: bool,
    /// For graph builds, true only when every module's application object hit.
    pub application_object: bool,
    pub prelude_object: bool,
    pub runtime_object: bool,
    pub launcher_object: bool,
    pub executable: bool,
}

/// Byte sizes of the material build products.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArtifactSizes {
    pub generated_c: u64,
    pub application_object: u64,
    pub prelude_object: u64,
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
    /// Engines supplied by the provider archive linked into this executable.
    pub provider_capabilities: crate::ProviderCapabilities,
    /// Engines the retained program actually requires. This can be a strict subset of the
    /// supplied set until a finer-grained packaged variant is available.
    pub required_provider_capabilities: crate::ProviderCapabilities,
    /// Every `parallelMap` call site and what the emitter decided for it, per module (its
    /// project-relative path; `<main>` for a single-program build), read back from the
    /// generated C the build compiled -- see `kiln_backend_c::parallel::SiteRecord`. Modules
    /// with no site are left out.
    pub parallel_sites: Vec<(String, Vec<kiln_backend_c::parallel::SiteRecord>)>,
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
