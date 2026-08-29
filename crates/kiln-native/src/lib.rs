//! Cached orchestration for Kiln's C native backend.
//!
//! The backend, runtime, cache, and platform toolchain deliberately live in separate crates.
//! This crate is the narrow coordinator that gives those pieces an ordered build graph:
//! generated module C, application object, reusable runtime object, launcher object, executable.

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
use kiln_ir::Program;
use kiln_module_graph::{ModuleGraph, ModuleGraphError};
pub use kiln_native_runtime::embedding_header;
use kiln_native_runtime::{
    EMBEDDING_C_ABI_VERSION, EMBEDDING_HOST_ABI_VERSION, NATIVE_RUNTIME_VERSION,
    RuntimeSourceError, launcher_source, runtime_source,
};
use kiln_native_toolchain::{
    BuildProfile as ToolchainProfile, CompileOptions, LinkOptions, ToolIdentity, Toolchain,
    ToolchainError,
};

static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);
static MATERIALIZE_COUNTER: AtomicU64 = AtomicU64::new(0);
static DISCOVERED_TOOLCHAIN: OnceLock<Result<Toolchain, ToolchainError>> = OnceLock::new();

const _: () = {
    assert!(EMBEDDING_C_ABI_VERSION == C_ABI_VERSION);
    assert!(EMBEDDING_HOST_ABI_VERSION == HOST_ABI_VERSION);
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
            Self::WorkerPanic { .. } | Self::SymbolCollision { .. } => None,
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
        let emit_elapsed = emit_started.elapsed();

        let workspace = WorkDirectory::create()?;
        let paths = SharedLibraryPaths::new(workspace.path());
        let identity = self.toolchain.identity().clone();
        let profile = options.profile;
        let module_abi = module_abi_identity();
        let embedding_abi = runtime_abi_identity();
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

        let link_options_digest = Digest::hash(
            format!(
                "shared-library\0{}\0system-math\0{}\0{}\0{}",
                profile.as_str(),
                shared_library_name(),
                shared_library_link_identity(&options.output),
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
            [object_digest],
        );
        let (library, library_hit, link_elapsed) = match self.cache.lookup(&library_key)? {
            Lookup::Hit(artifact) => (artifact.bytes, true, Duration::ZERO),
            Lookup::Miss => {
                materialize(&paths.module_o, &object, false)?;
                let mut link_options = LinkOptions {
                    profile: profile.toolchain(),
                    ..LinkOptions::default()
                };
                if cfg!(not(target_os = "windows")) {
                    link_options.libraries.push("m".to_owned());
                }
                configure_shared_library_link(&mut link_options, &options.output);
                let report = self.toolchain.link_shared_library(
                    std::slice::from_ref(&paths.module_o),
                    &paths.library,
                    &link_options,
                )?;
                let bytes = read(&paths.library, "read linked shared library")?;
                self.cache.put(&library_key, &bytes)?;
                (bytes, false, report.elapsed)
            }
        };
        materialize(&options.output, &library, false)?;

        Ok(NativeSharedLibraryReport {
            output: options.output.clone(),
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
                objects.push(paths.runtime_o.clone());
                objects.push(paths.launcher_o.clone());
                let mut link_options = LinkOptions {
                    profile: profile.toolchain(),
                    ..LinkOptions::default()
                };
                if cfg!(not(target_os = "windows")) {
                    link_options.libraries.push("m".to_owned());
                }
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
        let mut private_options = options.clone();
        private_options.output = private_output.clone();
        let mut build = self.build_graph(graph, &private_options)?;
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
            let provider_index = *indices.get(&dependency.target).ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::InternalMissingModule {
                    path: dependency.target.clone(),
                })
            })?;
            let export_index = modules[provider_index]
                .0
                .program
                .exports
                .iter()
                .position(|export| export.exported == import.imported)
                .ok_or_else(|| {
                    NativeError::Graph(ModuleGraphError::MissingExport {
                        importer: node.path.clone(),
                        provider: dependency.target.clone(),
                        name: import.imported.clone(),
                    })
                })?;
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
        output.push_str("    if (status != 0) return 1;\n");
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

struct SharedLibraryPaths {
    module_c: PathBuf,
    module_o: PathBuf,
    library: PathBuf,
}

struct GraphBuildPaths {
    module_c: Vec<PathBuf>,
    module_o: Vec<PathBuf>,
    runtime_c: PathBuf,
    runtime_o: PathBuf,
    launcher_c: PathBuf,
    launcher_o: PathBuf,
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

const fn shared_library_link_policy() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos-dynamiclib-install-name-rpath-v1"
    } else if cfg!(target_os = "linux") {
        "linux-shared-soname-v1"
    } else {
        "unsupported-shared-link-v1"
    }
}

fn configure_shared_library_link(options: &mut LinkOptions, output: &Path) {
    let identity = shared_library_link_identity(output);
    #[cfg(target_os = "macos")]
    options
        .extra_args
        .push(format!("-Wl,-install_name,{identity}").into());
    #[cfg(target_os = "linux")]
    options
        .extra_args
        .push(format!("-Wl,-soname,{identity}").into());
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = (options, identity);
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
        let program = stdout_program("embedded\0tail");
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
            !renamed.cache_hits.library,
            "the embedded install name or SONAME changes with the output basename"
        );

        let header = root.path.join("kiln-embedding.h");
        let host_source = root.path.join("host.c");
        let host_object = root.path.join("host.o");
        let host_executable = root.path.join("host");
        fs::write(&header, embedding_header()).expect("write embedding header");
        let mut source = String::from(
            r#"#include "kiln-embedding.h"
#include <stdio.h>
#include <string.h>

struct capture {
    char bytes[256];
    size_t length;
    int calls;
};

"#,
        );
        source.push_str("extern int ");
        source.push_str(&second.module_symbol);
        source.push_str(
            r#"(kiln_host *host, kiln_value *result);

static int capture_host_call(void *context, uint16_t operation, uint16_t argc,
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
        link_options
            .extra_args
            .push(format!("-Wl,-rpath,{}", root.path.display()).into());
        toolchain
            .link_executable(
                std::slice::from_ref(&host_object),
                &host_executable,
                &link_options,
            )
            .expect("link embedding host");
        let mut command = Command::new(&host_executable);
        #[cfg(target_os = "linux")]
        command.env("LD_LIBRARY_PATH", &root.path);
        #[cfg(target_os = "macos")]
        command.env("DYLD_LIBRARY_PATH", &root.path);
        let output = command.output().expect("run embedding host");
        assert!(
            output.status.success(),
            "embedding host failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"embedded\0tail\n");
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
            functions: Vec::new(),
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
