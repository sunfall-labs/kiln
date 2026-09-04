//! Cached orchestration for Kiln's C native backend.
//!
//! The backend, runtime, cache, and platform toolchain deliberately live in separate crates.
//! This crate is the narrow coordinator that gives those pieces an ordered build graph:
//! generated module C, application object, reusable runtime object, launcher object, executable.

mod number_provider;
pub mod shared_publication;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use kiln_artifact_cache::{
    ArtifactCache, ArtifactKey, ArtifactKind, Backend, BuildProfile as CacheProfile, CacheError,
    Digest, Lookup,
};
#[cfg(test)]
use kiln_backend_c::emit_module_with_logical_path;
use kiln_backend_c::{
    C_ABI_VERSION, C_GENERATOR_VERSION, EmissionPolicy, EmitError,
    emit_commonjs_module_with_logical_path_and_policy, emit_module_with_logical_path_and_policy,
    emit_runtime_module_with_logical_path, emit_with_policy,
};
use kiln_host_abi::HOST_ABI_VERSION;
pub use kiln_host_abi::HostValue;
use kiln_ir::{ExportBinding, ModuleProgram, Program};
use kiln_module_graph::{ModuleGraph, ModuleGraphError, ModuleSource, ResolvedExport};
use kiln_native_runtime::{
    EMBEDDING_C_ABI_VERSION, EMBEDDING_HOST_ABI_VERSION, EMBEDDING_MODULE_API_VERSION,
    NATIVE_RESULT_PATH_ENV, NATIVE_RUNTIME_VERSION, decode_native_result,
    embedding_module_api_symbol, launcher_source, runtime_source,
};
pub use kiln_native_runtime::{embedding_header, embedding_module_header};
use kiln_native_toolchain::{
    CompileOptions, CompilerKind, LinkOptions, ToolIdentity, Toolchain, ToolchainError,
};
use number_provider::{NumberProvider, PROVIDER_ABI_VERSION, PROVIDER_SCHEMA_VERSION};
use rayon::prelude::*;

static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);
static MATERIALIZE_COUNTER: AtomicU64 = AtomicU64::new(0);
static DISCOVERED_TOOLCHAIN: OnceLock<Result<Toolchain, ToolchainError>> = OnceLock::new();
static SHARED_PUBLICATION_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

const PRIVATE_ASYNC_ABI_VERSION: u32 = 6;
const NATIVE_PRELUDE_VERSION: u32 = 3;

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

mod options;
mod report;

pub use options::*;
pub use paths::shared_library_header_path;
pub use report::*;
mod materialize;
mod paths;
mod worker;

pub(crate) use materialize::*;
pub(crate) use paths::*;
pub(crate) use worker::*;

/// A configured cache and compiler driver reusable across daemon requests.
#[derive(Debug, Clone)]
pub struct NativeBuilder {
    cache: ArtifactCache,
    toolchain: Toolchain,
    number_provider: OnceLock<Result<NumberProvider, String>>,
}

fn cache_with_optional_shared(cache_root: impl Into<PathBuf>) -> ArtifactCache {
    let local = cache_root.into();
    if let Ok(shared_path) = std::env::var("KILN_SHARED_CACHE")
        && !shared_path.trim().is_empty()
    {
        let shared = PathBuf::from(shared_path.trim());
        ArtifactCache::with_layers([
            kiln_artifact_cache::CacheLayer::writable(shared),
            kiln_artifact_cache::CacheLayer::writable(local),
        ])
    } else {
        ArtifactCache::new(local)
    }
}

impl NativeBuilder {
    pub fn discover(cache_root: impl Into<PathBuf>) -> Result<Self, NativeError> {
        Ok(Self::new(
            cache_with_optional_shared(cache_root),
            Toolchain::discover()?,
        ))
    }

    /// Reuse one compiler probe for the lifetime of a resident process.
    pub fn discover_cached(cache_root: impl Into<PathBuf>) -> Result<Self, NativeError> {
        let toolchain = DISCOVERED_TOOLCHAIN
            .get_or_init(Toolchain::discover)
            .clone()?;
        Ok(Self::new(cache_with_optional_shared(cache_root), toolchain))
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
            format!("kiln-runtime-prelude-{NATIVE_PRELUDE_VERSION}"),
            Backend::Custom("native-prelude-source".to_owned()),
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
                    Backend::Custom("native-runtime-prelude".to_owned()),
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
                    Backend::Custom("native-shared-prelude".to_owned()),
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

    fn cached_number_provider(
        &self,
        target: &str,
    ) -> Result<(Arc<[u8]>, Digest, bool), NativeError> {
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
        match self.lookup_digest(&provider_key)? {
            Some((digest, _size)) => {
                debug_assert_eq!(digest, provider.archive_digest);
                Ok((Arc::clone(&provider.archive), digest, true))
            }
            None => {
                let stored = self.cache.put(&provider_key, &provider.archive)?;
                Ok((Arc::clone(&provider.archive), stored.digest, false))
            }
        }
    }

    /// Build one bytecode module and its separate host runtime into an executable.
    pub fn build(
        &self,
        program: &Program,
        options: &NativeBuildOptions,
    ) -> Result<NativeBuildReport, NativeError> {
        let total_started = Instant::now();
        let emit_started = Instant::now();
        let emission = options.realm.emission_executable();
        let module = emit_with_policy(program, emission.clone())?;
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

        let prelude_source = kiln_backend_c::runtime_prelude_source_with_policy(&emission);
        let prelude_source_key = key(
            ArtifactKind::GeneratedC,
            format!("kiln-runtime-prelude-{NATIVE_PRELUDE_VERSION}"),
            Backend::Custom("native-prelude-source".to_owned()),
            &runtime_abi,
            &target,
            CacheProfile::Debug,
            Digest::hash(b"portable-prelude-c11"),
            [Digest::hash(prelude_source.as_bytes())],
        );
        let (prelude_bytes, prelude_source_digest, _) =
            self.cached_payload(&prelude_source_key, prelude_source.as_bytes())?;

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
        let prelude_key = key(
            ArtifactKind::RuntimeLibrary,
            &compiler,
            Backend::Custom("native-runtime-prelude".to_owned()),
            &runtime_abi,
            &target,
            profile.cache(),
            object_options,
            [prelude_source_digest],
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

        let link_options_digest = Digest::hash(
            format!("{}\0system-math\0system-entropy-v1", profile.as_str()).as_bytes(),
        );
        if let Some(app_info) = self.lookup_digest(&application_key)?
            && let Some(prelude_info) = self.lookup_digest(&prelude_key)?
            && let Some(runtime_info) = self.lookup_digest(&runtime_key)?
            && let Some(launcher_info) = self.lookup_digest(&launcher_key)?
        {
            let candidate_executable_key = key(
                ArtifactKind::LinkedArtifact,
                &compiler,
                Backend::C,
                &runtime_abi,
                &target,
                profile.cache(),
                link_options_digest,
                [
                    app_info.0,
                    prelude_info.0,
                    runtime_info.0,
                    launcher_info.0,
                    number_provider_digest,
                ],
            );
            if let Lookup::Hit(artifact) = self.lookup(&candidate_executable_key)? {
                debug_assert_eq!(artifact.digest, Digest::hash(&artifact.bytes));
                materialize(&options.output, &artifact.bytes, true)?;
                return Ok(NativeBuildReport {
                    output: options.output.clone(),
                    module_symbol: module.symbol,
                    tool: identity,
                    cache_hits: CacheHits {
                        generated_c: generated_c_hit,
                        application_object: true,
                        runtime_object: true,
                        launcher_object: true,
                        executable: true,
                    },
                    sizes: ArtifactSizes {
                        generated_c: module_source.len() as u64,
                        application_object: app_info.1,
                        runtime_object: runtime_info.1,
                        launcher_object: launcher_info.1,
                        executable: artifact.bytes.len() as u64,
                    },
                    timings: BuildTimings {
                        emit: emit_elapsed,
                        compile: Duration::ZERO,
                        link: Duration::ZERO,
                        total: total_started.elapsed(),
                    },
                    module_artifacts: vec![ModuleArtifactInfo {
                        logical_path: None,
                        generated_c_cache_hit: generated_c_hit,
                        object_cache_hit: true,
                        generated_c_bytes: module_source.len() as u64,
                        object_bytes: app_info.1,
                    }],
                    runtime_generated_c_cache_hit,
                    launcher_generated_c_cache_hit,
                });
            }
        }

        let compile_options = CompileOptions {
            profile: profile.toolchain(),
            ..CompileOptions::default()
        };
        let compile_started = Instant::now();
        let (((application_result, prelude_result), runtime_result), launcher_result) = rayon::join(
            || {
                rayon::join(
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
                                run_worker("prelude compile", || {
                                    self.object(
                                        &prelude_key,
                                        &prelude_bytes,
                                        &paths.prelude_c,
                                        &paths.prelude_o,
                                        &compile_options,
                                    )
                                })
                            },
                        )
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
        let (prelude_object, prelude_digest, prelude_hit, _prelude_elapsed) = prelude_result?;
        let (runtime_object, runtime_digest, runtime_hit, _runtime_elapsed) = runtime_result?;
        let (launcher_object, launcher_digest, launcher_hit, _launcher_elapsed) = launcher_result?;
        let compile_elapsed = if application_hit && prelude_hit && runtime_hit && launcher_hit {
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
                prelude_digest,
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
                    materialize(&paths.prelude_o, &prelude_object, false)?;
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
                            paths.prelude_o.clone(),
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
        let runtime_abi = runtime_abi_identity();
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
        let prelude_source =
            kiln_backend_c::runtime_prelude_source_with_policy(&options.realm.emission());
        let prelude_source_key = key(
            ArtifactKind::GeneratedC,
            format!("kiln-runtime-prelude-{NATIVE_PRELUDE_VERSION}"),
            Backend::Custom("native-prelude-source".to_owned()),
            &runtime_abi,
            &target,
            CacheProfile::Debug,
            Digest::hash(b"portable-prelude-c11"),
            [Digest::hash(prelude_source.as_bytes())],
        );
        let (prelude_bytes, prelude_source_digest, _) =
            self.cached_payload(&prelude_source_key, prelude_source.as_bytes())?;
        let prelude_key = key(
            ArtifactKind::NativeObject,
            &compiler,
            Backend::Custom("native-shared-prelude".to_owned()),
            &embedding_abi,
            &target,
            profile.cache(),
            object_options,
            [prelude_source_digest],
        );

        if let Some(object_info) = self.lookup_digest(&object_key)?
            && let Some(prelude_info) = self.lookup_digest(&prelude_key)?
        {
            let link_inputs = [object_info.0, prelude_info.0, number_provider_digest];
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
            if let Lookup::Hit(artifact) = self.lookup(&library_key)? {
                publish_shared_artifacts_with_symbols(
                    &options.output,
                    &loader,
                    header_source.as_bytes(),
                    &artifact.bytes,
                    &module.symbol,
                    &module_api_symbol,
                )?;
                return Ok(NativeSharedLibraryReport {
                    output: options.output.clone(),
                    loader,
                    header,
                    module_symbol: module.symbol,
                    module_api_symbol,
                    tool: identity,
                    cache_hits: NativeSharedLibraryCacheHits {
                        generated_c: generated_c_hit,
                        object: true,
                        library: true,
                    },
                    sizes: NativeSharedLibrarySizes {
                        generated_c: module_source.len() as u64,
                        object: object_info.1,
                        library: artifact.bytes.len() as u64,
                    },
                    timings: BuildTimings {
                        emit: emit_elapsed,
                        compile: Duration::ZERO,
                        link: Duration::ZERO,
                        total: total_started.elapsed(),
                    },
                });
            }
        }

        let compile_options = CompileOptions {
            profile: profile.toolchain(),
            position_independent: true,
            ..CompileOptions::default()
        };
        let compile_started = Instant::now();
        let (module_compile, prelude_compile) = rayon::join(
            || {
                self.object(
                    &object_key,
                    &module_source,
                    &paths.module_c,
                    &paths.module_o,
                    &compile_options,
                )
            },
            || {
                self.object(
                    &prelude_key,
                    &prelude_bytes,
                    &paths.prelude_c,
                    &paths.prelude_o,
                    &compile_options,
                )
            },
        );
        let (object, object_digest, object_hit, object_elapsed) = module_compile?;
        let (prelude_object, prelude_digest, prelude_hit, prelude_elapsed) = prelude_compile?;
        let compile_elapsed = if object_hit && prelude_hit {
            Duration::ZERO
        } else {
            object_elapsed
                .max(prelude_elapsed)
                .max(compile_started.elapsed())
        };

        let link_inputs = [object_digest, prelude_digest, number_provider_digest];
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
                materialize(&paths.prelude_o, &prelude_object, false)?;
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
                    self.toolchain.identity().kind,
                )?;
                let objects = [paths.module_o.clone(), paths.prelude_o.clone()];
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
        let symbol_list = modules
            .iter()
            .map(|(_, emitted)| &emitted.symbol)
            .collect::<Vec<_>>();
        let paths = SharedGraphLibraryPaths::new(workspace.path(), &symbol_list);
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
        let prelude_source =
            kiln_backend_c::runtime_prelude_source_with_policy(&options.realm.emission());
        let prelude_source_key = key(
            ArtifactKind::GeneratedC,
            format!("kiln-runtime-prelude-{NATIVE_PRELUDE_VERSION}"),
            Backend::Custom("native-prelude-source".to_owned()),
            &embedding_abi,
            &target,
            CacheProfile::Debug,
            Digest::hash(b"portable-prelude-c11"),
            [Digest::hash(prelude_source.as_bytes())],
        );
        let (prelude_bytes, prelude_source_digest, _) =
            self.cached_payload(&prelude_source_key, prelude_source.as_bytes())?;
        let shared_prelude_object_options = Digest::hash(
            format!(
                "shared-library\0pic\0c11\0{}\0{}\0generator-{C_GENERATOR_VERSION}",
                identity.kind,
                profile.as_str()
            )
            .as_bytes(),
        );
        let prelude_key = key(
            ArtifactKind::NativeObject,
            &compiler,
            Backend::Custom("native-shared-prelude".to_owned()),
            &shared_embedding_abi_identity(),
            &target,
            profile.cache(),
            shared_prelude_object_options,
            [prelude_source_digest],
        );

        let mut module_object_infos = Vec::with_capacity(module_keys.len());
        let mut all_modules_cached = true;
        for key in &module_keys {
            match self.lookup_digest(key)? {
                Some(info) => module_object_infos.push(info),
                None => {
                    all_modules_cached = false;
                    break;
                }
            }
        }
        if all_modules_cached
            && let Some(wrapper_info) = self.lookup_digest(&wrapper_object_key)?
            && let Some(prelude_info) = self.lookup_digest(&prelude_key)?
        {
            let mut link_inputs = module_object_infos
                .iter()
                .map(|(digest, _)| *digest)
                .collect::<Vec<_>>();
            link_inputs.push(number_provider_digest);
            link_inputs.push(prelude_info.0);
            link_inputs.push(wrapper_info.0);
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
                link_inputs,
            );
            if let Lookup::Hit(artifact) = self.lookup(&library_key)? {
                publish_shared_artifacts_with_symbols(
                    &options.output,
                    &loader,
                    header_source.as_bytes(),
                    &artifact.bytes,
                    &wrapper_symbol,
                    &module_api_symbol,
                )?;
                return Ok(NativeSharedLibraryReport {
                    output: options.output.clone(),
                    loader,
                    header,
                    module_symbol: wrapper_symbol,
                    module_api_symbol,
                    tool: identity,
                    cache_hits: NativeSharedLibraryCacheHits {
                        generated_c: module_sources.iter().all(|(_, _, hit)| *hit),
                        object: true,
                        library: true,
                    },
                    sizes: NativeSharedLibrarySizes {
                        generated_c: module_sources
                            .iter()
                            .map(|(source, _, _)| source.len() as u64)
                            .sum::<u64>()
                            + wrapper_bytes.len() as u64,
                        object: module_object_infos
                            .iter()
                            .map(|(_, size)| *size)
                            .sum::<u64>()
                            + wrapper_info.1,
                        library: artifact.bytes.len() as u64,
                    },
                    timings: BuildTimings {
                        emit: emit_elapsed,
                        compile: Duration::ZERO,
                        link: Duration::ZERO,
                        total: total_started.elapsed(),
                    },
                });
            }
        }

        let compile_started = Instant::now();
        let (module_objects, other_objects) = rayon::join(
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
                rayon::join(
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
                    || {
                        run_worker("shared graph prelude compile", || {
                            self.object(
                                &prelude_key,
                                &prelude_bytes,
                                &paths.prelude_c,
                                &paths.prelude_o,
                                &wrapper_compile_options,
                            )
                        })
                    },
                )
            },
        );
        let module_objects = module_objects?;
        let (wrapper_object, prelude_object) = other_objects;
        let wrapper_object = wrapper_object?;
        let prelude_object = prelude_object?;
        let compile_elapsed = if module_objects.iter().all(|(_, _, hit)| *hit)
            && wrapper_object.2
            && prelude_object.2
        {
            Duration::ZERO
        } else {
            compile_started.elapsed()
        };

        let mut link_inputs = module_objects
            .iter()
            .map(|(_, digest, _)| *digest)
            .collect::<Vec<_>>();
        link_inputs.push(number_provider_digest);
        link_inputs.push(prelude_object.1);
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
                let mut objects = Vec::with_capacity(modules.len() + 2);
                for (index, (object, _, _)) in module_objects.iter().enumerate() {
                    materialize(&paths.module_o[index], object, false)?;
                    objects.push(paths.module_o[index].clone());
                }
                materialize(&paths.prelude_o, &prelude_object.0, false)?;
                objects.push(paths.prelude_o.clone());
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
                    self.toolchain.identity().kind,
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
        let emission = options.realm.emission_executable();
        let mut modules = Vec::with_capacity(graph.module_count());
        let mut logical_paths = Vec::with_capacity(graph.module_count());
        let mut symbols = std::collections::HashSet::with_capacity(graph.module_count());
        for node in graph.modules() {
            let logical_path = logical_module_path(graph.project_root(), &node.path)?;
            let emitted = emit_native_graph_module_with_policy(node, &logical_path, &emission)?;
            if !symbols.insert(emitted.symbol.clone()) {
                return Err(NativeError::SymbolCollision {
                    symbol: emitted.symbol,
                });
            }
            logical_paths.push(logical_path);
            modules.push((node, emitted));
        }
        let launcher = graph_launcher_source(graph, &modules, &options.realm)?;
        let emit_elapsed = emit_started.elapsed();
        let runtime = runtime_source();

        let workspace = WorkDirectory::create()?;
        let symbol_list = modules
            .iter()
            .map(|(_, emitted)| &emitted.symbol)
            .collect::<Vec<_>>();
        let paths = GraphBuildPaths::new(workspace.path(), &symbol_list);
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
        let prelude_source = kiln_backend_c::runtime_prelude_source_with_policy(&emission);
        let prelude_source_key = key(
            ArtifactKind::GeneratedC,
            format!("kiln-runtime-prelude-{NATIVE_PRELUDE_VERSION}"),
            Backend::Custom("native-prelude-source".to_owned()),
            &runtime_abi,
            &target,
            CacheProfile::Debug,
            Digest::hash(b"portable-prelude-c11"),
            [Digest::hash(prelude_source.as_bytes())],
        );
        let (prelude_bytes, prelude_source_digest, _) =
            self.cached_payload(&prelude_source_key, prelude_source.as_bytes())?;

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
        let prelude_key = key(
            ArtifactKind::RuntimeLibrary,
            &compiler,
            Backend::Custom("native-runtime-prelude".to_owned()),
            &runtime_abi,
            &target,
            profile.cache(),
            object_options,
            [prelude_source_digest],
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
            Backend::Custom("native-graph-launcher".to_owned()),
            &runtime_abi,
            &target,
            profile.cache(),
            object_options,
            [launcher_source_digest],
        );

        let link_options_digest = Digest::hash(
            format!("{}\0system-math\0system-entropy-v1", profile.as_str()).as_bytes(),
        );

        let mut module_object_infos = Vec::with_capacity(module_keys.len());
        let mut all_modules_cached = true;
        for key in &module_keys {
            match self.lookup_digest(key)? {
                Some(info) => module_object_infos.push(info),
                None => {
                    all_modules_cached = false;
                    break;
                }
            }
        }
        if all_modules_cached
            && let Some(prelude_info) = self.lookup_digest(&prelude_key)?
            && let Some(runtime_info) = self.lookup_digest(&runtime_key)?
            && let Some(launcher_info) = self.lookup_digest(&launcher_key)?
        {
            let mut link_inputs = module_object_infos
                .iter()
                .map(|(digest, _)| *digest)
                .collect::<Vec<_>>();
            link_inputs.push(prelude_info.0);
            link_inputs.push(runtime_info.0);
            link_inputs.push(launcher_info.0);
            link_inputs.push(number_provider_digest);
            let candidate_executable_key = key(
                ArtifactKind::LinkedArtifact,
                &compiler,
                Backend::C,
                &runtime_abi,
                &target,
                profile.cache(),
                link_options_digest,
                link_inputs,
            );
            if let Lookup::Hit(artifact) = self.lookup(&candidate_executable_key)? {
                debug_assert_eq!(artifact.digest, Digest::hash(&artifact.bytes));
                materialize(&options.output, &artifact.bytes, true)?;
                let mut module_artifacts = Vec::with_capacity(modules.len());
                for (index, _) in modules.iter().enumerate() {
                    module_artifacts.push(ModuleArtifactInfo {
                        logical_path: Some(logical_paths[index].clone()),
                        generated_c_cache_hit: module_sources[index].2,
                        object_cache_hit: true,
                        generated_c_bytes: module_sources[index].0.len() as u64,
                        object_bytes: module_object_infos[index].1,
                    });
                }
                let total_elapsed = total_started.elapsed();
                return Ok(NativeBuildReport {
                    output: options.output.clone(),
                    module_symbol: modules[0].1.symbol.clone(),
                    tool: identity,
                    cache_hits: CacheHits {
                        generated_c: module_sources.iter().all(|(_, _, hit)| *hit),
                        application_object: true,
                        runtime_object: true,
                        launcher_object: true,
                        executable: true,
                    },
                    sizes: ArtifactSizes {
                        generated_c: module_sources
                            .iter()
                            .map(|(source, _, _)| source.len() as u64)
                            .sum(),
                        application_object: module_object_infos.iter().map(|(_, size)| *size).sum(),
                        runtime_object: runtime_info.1,
                        launcher_object: launcher_info.1,
                        executable: artifact.bytes.len() as u64,
                    },
                    timings: BuildTimings {
                        emit: emit_elapsed,
                        compile: Duration::ZERO,
                        link: Duration::ZERO,
                        total: total_elapsed,
                    },
                    module_artifacts,
                    runtime_generated_c_cache_hit,
                    launcher_generated_c_cache_hit,
                });
            }
        }

        let (module_objects, ((prelude_object, runtime_object), launcher_object)) = rayon::join(
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
                        rayon::join(
                            || {
                                run_worker("prelude compile", || {
                                    self.object(
                                        &prelude_key,
                                        &prelude_bytes,
                                        &paths.prelude_c,
                                        &paths.prelude_o,
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
                )
            },
        );
        let module_objects = module_objects?;
        let (prelude_object, prelude_digest, prelude_hit, _) = prelude_object?;
        let (runtime_object, runtime_digest, runtime_hit, _) = runtime_object?;
        let (launcher_object, launcher_digest, launcher_hit, _) = launcher_object?;
        let compile_elapsed = if module_objects.iter().all(|(_, _, hit)| *hit)
            && prelude_hit
            && runtime_hit
            && launcher_hit
        {
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
        link_inputs.push(prelude_digest);
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
                let mut objects = Vec::with_capacity(modules.len() + 3);
                for (index, (object, _, _)) in module_objects.iter().enumerate() {
                    materialize(&paths.module_o[index], object, false)?;
                    objects.push(paths.module_o[index].clone());
                }
                materialize(&paths.prelude_o, &prelude_object, false)?;
                materialize(&paths.runtime_o, &runtime_object, false)?;
                materialize(&paths.launcher_o, &launcher_object, false)?;
                materialize(&paths.number_provider_a, &number_provider, false)?;
                objects.push(paths.prelude_o.clone());
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
        let output = run_executable(
            Command::new(&private_output).env(NATIVE_RESULT_PATH_ENV, &result_path),
            &private_output,
        )?;
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
        let output = run_executable(
            Command::new(&private_output).env(NATIVE_RESULT_PATH_ENV, &result_path),
            &private_output,
        )?;
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

    fn lookup_digest(&self, key: &ArtifactKey) -> Result<Option<(Digest, u64)>, NativeError> {
        #[cfg(test)]
        self.assert_live_cache_identity(key);
        Ok(self.cache.lookup_digest(key)?)
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

/// Whether an unbuilt member of `builtin` is a *function* in the interpreter.
///
/// This decides how a member with nothing behind it is published, and the two answers are
/// genuinely different rather than two spellings of one thing.
///
/// A function is published as a callable that refuses when it is called. That keeps
/// `typeof fs.readFileSync === "function"` agreeing with the interpreter -- which is what a
/// library feature-detects on -- and, more to the point, keeps
/// `const { readFileSync } = require("fs")` working. Destructuring reads every name it mentions
/// whether or not the program calls any of them, so a refusal on the *read* stops a program at
/// its first import over a function it was never going to use. `postcss` is exactly that program.
///
/// Anything else has no such value to hand back, and a callable would be a lie about `typeof`.
/// Those refuse on the read instead. There is one today: `node:path`'s `posix`, an object in the
/// interpreter and the module itself, whose identity a substitute cannot have.
///
/// The default is "function", because almost every member of every node builtin is one and a
/// wrong answer in that direction is a `typeof` this backend already agrees with.
fn native_builtin_export_is_callable(
    builtin: kiln_module_resolver::NodeBuiltin,
    export_name: &str,
) -> bool {
    use kiln_module_resolver::NodeBuiltin;

    // Only unbuilt members reach here, and every other non-function on the surface -- `path.sep`,
    // `path.delimiter`, `os.EOL`, `os.devNull`, `fs.promises`, `buffer.constants`,
    // `perf_hooks.performance` -- is answered, so the question does not arise for them. A member
    // dropped from a support list later would default to "function", which is right for a
    // function and wants a row here if it is not.
    !matches!((builtin, export_name), (NodeBuiltin::Path, "posix"))
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
        // `METHODS` is a constant array rather than a function, so it costs nothing to answer and
        // a placeholder served it worst: the unmaterialized-callable fallback made
        // `typeof http.METHODS` "function" against the interpreter's "object", which is a wrong
        // answer rather than a refusal.
        NodeBuiltin::Http => &[
            "METHODS",
            "Server",
            "ServerResponse",
            "createServer",
            "default",
            "get",
            "request",
        ],
        NodeBuiltin::Util => &[
            "TextDecoder",
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
        // `EOL` and `devNull` are constants rather than readings of the machine, so answering them
        // costs nothing and reads nothing -- and they are the two members of `node:os` that are
        // not functions, which makes them the two that a refusal would serve worst. `os.EOL` is
        // read far more often than it is called, `text.split(os.EOL)` being the whole of why, and
        // it had been answering `undefined` here against the interpreter's "\n".
        NodeBuiltin::Os => &["EOL", "default", "devNull", "hostname", "networkInterfaces"],
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
        NodeBuiltin::Path => &[
            "basename",
            "default",
            "delimiter",
            "dirname",
            "extname",
            "isAbsolute",
            "join",
            "posix",
            "relative",
            "resolve",
            "sep",
        ],
        NodeBuiltin::Assert => &["default", "ok", "strict"],
        NodeBuiltin::WorkerThreads => &["Worker", "default"],
        NodeBuiltin::Module => &["createRequire", "default"],
        NodeBuiltin::QueryString => &["decode", "default", "parse"],
        NodeBuiltin::Url => &["URL", "default", "fileURLToPath", "parse", "pathToFileURL"],
        // Every name the shared ABI declares: `Buffer`'s statics are real methods in this
        // runtime, so there is nothing here to hold an alignment slot for.
        NodeBuiltin::Buffer => &[
            "Buffer",
            "alloc",
            "allocUnsafe",
            "allocUnsafeSlow",
            "byteLength",
            "concat",
            "constants",
            "default",
            "from",
            "isBuffer",
        ],
        NodeBuiltin::AsyncHooks => &["AsyncLocalStorage", "AsyncResource", "default"],
        NodeBuiltin::Crypto => &["createHash", "default", "randomUUID"],
        _ => &[],
    }
}

fn emit_native_graph_module(
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &NativeRealmPolicy,
) -> Result<kiln_backend_c::CModule, NativeError> {
    emit_native_graph_module_with_policy(node, logical_path, &policy.emission())
}

fn emit_native_graph_module_with_policy(
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &EmissionPolicy,
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
            policy.clone(),
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
            policy.clone(),
        )?),
    }
}

/// `node:http`'s materializer, which is the one body that is generated rather than written out.
///
/// `METHODS` is the string list both engines have to publish identically, and the shared
/// list lives in `kiln-node-abi`. Building the C from that list is what makes the two engines one
/// fact: a method added there reaches the interpreter's array and this array in the same commit,
/// where a hand-copied C literal would be a second list that agrees until it does not.
///
/// A `LazyLock` rather than a `String` return type so the other twenty-odd bodies stay `&'static
/// str` literals; only this one needs to be computed.
static NODE_HTTP_BODY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let mut body = String::from(
        r#"kiln_value server = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_SERVER);
kiln_value server_response = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE);
kiln_value create_server = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER);
kiln_value get = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_GET);
kiln_value request = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_REQUEST);
kiln_value methods;
kiln_value method_name;
kiln_value namespace;
int status = kiln_c99_new_array(&methods);
if (status != KILN_C99_OK) return status;
"#,
    );
    for method in kiln_node_abi::HTTP_METHODS {
        writeln!(
            body,
            "method_name = kiln_c99_copy_text({}, {});",
            native_c_string(method),
            method.len()
        )
        .unwrap();
        writeln!(
            body,
            "if (method_name.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;"
        )
        .unwrap();
        writeln!(body, "status = kiln_c99_array_push(methods, method_name);").unwrap();
        writeln!(body, "if (status != KILN_C99_OK) return status;").unwrap();
    }
    body.push_str(
        r#"status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "METHODS", 7, methods, 0, NULL);
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
exports[1] = methods;
exports[2] = server;
exports[3] = server_response;
exports[4] = create_server;
exports[5] = namespace;
exports[6] = get;
exports[7] = request;"#,
    );
    body
});

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
        // addition to the table fails a test here instead of miscompiling a server. The one
        // remaining gated name -- `IncomingMessage` -- holds `undefined` because
        // `native_builtin_supports_export` refuses it as a named import; it is a slot to keep
        // alignment, not an export.
        NodeBuiltin::Http => Some(&NODE_HTTP_BODY),
        NodeBuiltin::Util => Some(
            r#"kiln_value text_decoder = kiln_c99_builtin(KILN_C99_BUILTIN_TEXT_DECODER);
kiln_value text_encoder = kiln_c99_builtin(KILN_C99_BUILTIN_TEXT_ENCODER);
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
status = kiln_c99_put(host, namespace, "TextDecoder", 11, text_decoder, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "TextEncoder", 11, text_encoder, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = text_decoder;
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
status = kiln_c99_put(host, namespace, "EOL", 3, kiln_c99_copy_text("\n", 1), 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "devNull", 7, kiln_c99_copy_text("/dev/null", 9), 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = kiln_c99_copy_text("\n", 1);
exports[1] = kiln_c99_undefined();
exports[2] = kiln_c99_undefined();
exports[3] = kiln_c99_undefined();
exports[4] = namespace;
exports[5] = kiln_c99_copy_text("/dev/null", 9);
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
        // `require("node:perf_hooks").performance === globalThis.performance` in node, and a
        // second object built here made them two. That is not cosmetic: the pair is how a library
        // decides it has already installed its own timing shim, and two objects make it install
        // twice. The realm's own `performance` builtin is the one value, so both spellings reach
        // it and `now` comes off the same table the global does.
        NodeBuiltin::PerfHooks => Some(
            r#"kiln_value performance = kiln_c99_builtin(KILN_C99_BUILTIN_PERFORMANCE);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
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
            r#"kiln_value basename = kiln_c99_method(KILN_C99_METHOD_NODE_PATH_BASENAME);
kiln_value dirname = kiln_c99_method(KILN_C99_METHOD_NODE_PATH_DIRNAME);
kiln_value extname = kiln_c99_method(KILN_C99_METHOD_NODE_PATH_EXTNAME);
kiln_value is_absolute = kiln_c99_method(KILN_C99_METHOD_NODE_PATH_IS_ABSOLUTE);
kiln_value join = kiln_c99_method(KILN_C99_METHOD_NODE_PATH_JOIN);
kiln_value relative = kiln_c99_method(KILN_C99_METHOD_NODE_PATH_RELATIVE);
kiln_value resolve = kiln_c99_method(KILN_C99_METHOD_NODE_PATH_RESOLVE);
kiln_value delimiter = kiln_c99_copy_text(":", 1);
kiln_value sep = kiln_c99_copy_text("/", 1);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "basename", 8, basename, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "default", 7, namespace, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "delimiter", 9, delimiter, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "dirname", 7, dirname, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "extname", 7, extname, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "isAbsolute", 10, is_absolute, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "join", 4, join, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "posix", 5, namespace, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "relative", 8, relative, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "resolve", 7, resolve, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "sep", 3, sep, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = basename;
exports[1] = namespace;
exports[2] = delimiter;
exports[3] = dirname;
exports[4] = extname;
exports[5] = kiln_c99_undefined();
exports[6] = is_absolute;
exports[7] = join;
exports[8] = kiln_c99_undefined();
exports[9] = kiln_c99_undefined();
exports[10] = namespace;
exports[11] = relative;
exports[12] = resolve;
exports[13] = sep;"#,
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
        // `node:module`'s default is the `Module` class, the same shape `node:stream`'s default
        // is the `Stream` class: node answers `typeof` "function" for both, and the interpreter
        // does too. This built a fresh namespace object instead, so the two engines read the same
        // bytecode two ways -- "object" here against "function" there -- and a bundle that
        // feature-detects `typeof require("node:module") === "function"` took the other branch.
        // `createRequire` hangs off the class, which is where node has it.
        NodeBuiltin::Module => Some(
            r#"kiln_value create_require = kiln_c99_method(KILN_C99_METHOD_NODE_MODULE_CREATE_REQUIRE);
kiln_value module_class = kiln_c99_method(KILN_C99_METHOD_NODE_MODULE);
int status = kiln_c99_put(host, module_class, "createRequire", 13, create_require, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = create_require;
exports[1] = module_class;"#,
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
            // The seven statics are the same method codes the `Buffer` global answers, because
            // they are the same functions: the interpreter hands `require("node:buffer").alloc`
            // and `Buffer.alloc` the same value, and two different codes here would be two
            // different functions to `===`.
            r#"kiln_value constructor = kiln_c99_builtin(KILN_C99_BUILTIN_BUFFER);
kiln_value constants;
int status = kiln_c99_new_object(&constants);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, constants, "MAX_STRING_LENGTH", 17, kiln_c99_number(536870888.0), 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, constructor, "constants", 9, constants, 0, NULL);
if (status != KILN_C99_OK) return status;
kiln_value alloc = kiln_c99_method(KILN_C99_METHOD_BUFFER_ALLOC);
kiln_value byte_length = kiln_c99_method(KILN_C99_METHOD_BUFFER_BYTE_LENGTH);
kiln_value concat = kiln_c99_method(KILN_C99_METHOD_BUFFER_CONCAT);
kiln_value from = kiln_c99_method(KILN_C99_METHOD_BUFFER_FROM);
kiln_value is_buffer = kiln_c99_method(KILN_C99_METHOD_BUFFER_IS_BUFFER);
kiln_value namespace;
status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "Buffer", 6, constructor, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "alloc", 5, alloc, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "allocUnsafe", 11, alloc, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "allocUnsafeSlow", 15, alloc, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "byteLength", 10, byte_length, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "concat", 6, concat, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "constants", 9, constants, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "from", 4, from, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "isBuffer", 8, is_buffer, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = constructor;
exports[1] = alloc;
exports[2] = alloc;
exports[3] = alloc;
exports[4] = byte_length;
exports[5] = concat;
exports[6] = constants;
/* `default` is the module namespace, not the constructor: that is what the interpreter answers,
 * and `typeof require("node:buffer").default` is "object" there. It read "function" here. */
exports[7] = namespace;
exports[8] = from;
exports[9] = is_buffer;"#,
        ),
        NodeBuiltin::AsyncHooks => Some(
            r#"kiln_value als = kiln_c99_method(KILN_C99_METHOD_NODE_ASYNC_LOCAL_STORAGE);
kiln_value resource = kiln_c99_method(KILN_C99_METHOD_NODE_ASYNC_RESOURCE);
kiln_value namespace;
int status = kiln_c99_new_object(&namespace);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "AsyncLocalStorage", 17, als, 0, NULL);
if (status != KILN_C99_OK) return status;
status = kiln_c99_put(host, namespace, "AsyncResource", 13, resource, 0, NULL);
if (status != KILN_C99_OK) return status;
exports[0] = als;
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
    match cache.lookup_digest(key)? {
        Some((digest, size)) => {
            let expected_digest = Digest::hash(expected);
            if digest != expected_digest || size != expected.len() as u64 {
                match cache.lookup(key)? {
                    Lookup::Hit(artifact) if artifact.bytes != expected => {
                        return Err(CacheError::Corrupt {
                            path: artifact.path,
                            reason: format!(
                                "cached payload digest {} does not match deterministic expected digest {}",
                                artifact.digest.to_hex(),
                                expected_digest.to_hex()
                            ),
                        }
                        .into());
                    }
                    _ => {}
                }
            }
            Ok((expected.to_vec(), digest, true))
        }
        None => {
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
            // A name the materializer fills is published with its value; a name it does not is
            // published as a *refusal*, so that reading it stops the program and the instruction
            // says which member was asked for.
            //
            // This loop used to publish every ABI name unconditionally, and the slots the
            // materializer had left as `kiln_c99_undefined()` went out with it. That is where the
            // two halves of the loader stopped agreeing: a static `import { dirname }` was
            // refused ahead of time by `validate_native_node_builtins`, while
            // `require("path").dirname` -- which is what an esbuild bundle emits -- handed back
            // `undefined` and let the program run on. `postcss` walked past its own
            // `resolve && isAbsolute` guard on the strength of it and died three frames later on
            // "a undefined is not a function", with nothing anywhere naming `dirname`.
            for export in abi.exports {
                let index =
                    export_index(export).ok_or(NativeError::UnsupportedNodeBuiltinExport {
                        specifier: builtin.specifier(),
                        export_name: (*export).to_owned(),
                    })?;
                if native_builtin_supports_export(builtin, export) {
                    writeln!(body, "commonjs_status = kiln_c99_put(host, commonjs_namespace, {}, {}, exports[{index}], 0, NULL);", native_c_string(export), export.len()).unwrap();
                } else if native_builtin_export_is_callable(builtin, export) {
                    writeln!(body, "commonjs_status = kiln_c99_put(host, commonjs_namespace, {}, {}, kiln_c99_method(KILN_C99_METHOD_NODE_BUILTIN_UNMATERIALIZED), 0, NULL);", native_c_string(export), export.len()).unwrap();
                } else {
                    writeln!(
                        body,
                        "commonjs_status = kiln_c99_put_refused(commonjs_namespace, {}, {});",
                        native_c_string(export),
                        export.len()
                    )
                    .unwrap();
                }
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
    policy: &NativeRealmPolicy,
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
#include <stdlib.h>
#include <string.h>
#ifndef _WIN32
#include <sys/wait.h>
#include <unistd.h>
#endif

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
extern void kiln_cli_report_failure_in(int status, kiln_value value, const char *phase);
extern int kiln_realm_create(uint64_t *out_realm);
extern int kiln_realm_enter(uint64_t realm, kiln_realm_entry_fn entry, void *context);
extern int kiln_realm_set_argv(uint64_t realm, uint64_t argc, const char *const *argv);
extern int kiln_realm_use_wall_clock(uint64_t realm);
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
        r#"    return 0;
}

/* argc/argv reach the realm before it is entered, so the first `process` a module builds
 * already has the command line. The strings are copied on the other side of this call. */
static int kiln_cli_run_realm(int argc, char **argv, int writes_result) {
    kiln_graph_context graph = {0};
    uint64_t realm = 0;
    /* A refusal that reaches here has no diagnostic of its own -- the realm stopped before it
     * could leave one -- so the launcher says which of the three phases was running. */
    const char *phase = "realm creation";
    int status;
    int destroy_status;
    graph.result.tag = KILN_VALUE_UNDEFINED;
    graph.entry_result.tag = KILN_VALUE_UNDEFINED;
    graph.host.call = kiln_cli_host_call;
    graph.host.context = NULL;
    status = kiln_realm_create(&realm);
    if (status == 0 && argc > 0)
        status = kiln_realm_set_argv(realm, (uint64_t)argc, (const char *const *)argv);
"#,
    );
    if policy.host_clock {
        // ADR 0044: a shipped realm keeps wall time, and the switch happens before the realm is
        // entered so no timer is ever armed against the virtual clock.
        output.push_str("    if (status == 0) status = kiln_realm_use_wall_clock(realm);\n");
    }
    output.push_str(
        r##"    if (status == 0) { phase = "module body"; status = kiln_realm_enter(realm, kiln_graph_enter, &graph); }
    if (status == 0) { phase = "the turn loop"; status = kiln_realm_drive(realm, &graph.host, &graph.result); }
    if (status != 0) kiln_cli_report_failure_in(status, graph.result, phase);
    if (writes_result && status == 0 && kiln_cli_write_result(graph.entry_result) != 0) status = 1;
    destroy_status = kiln_realm_destroy(realm);
    if (status == 0) status = destroy_status;
    return status;
}

/* How many realms to run, read from the environment rather than fixed at build time.
 *
 * A server binds with SO_REUSEPORT, so N of these share a port and the kernel spreads the accepts
 * across them. Reading the count at startup keeps one artifact correct on every machine it is
 * copied to, which is the point of shipping a binary; baking it in at `kiln build` would tie the
 * artifact to the box that built it.
 *
 * Absent, unparseable or below one means one realm -- exactly the behaviour before this existed. */
static long kiln_cli_worker_count(void) {
    const char *setting = getenv("KILN_WORKERS");
    char *end = NULL;
    long count;
    if (setting == NULL || *setting == '\0') return 1;
    count = strtol(setting, &end, 10);
    if (end == setting || *end != '\0' || count < 1) return 1;
    if (count > 1024) count = 1024;
    return count;
}

/* Extra realms are separate *processes*, not threads, and that is a constraint rather than a
 * preference. A module's namespaces live in `kiln_namespaces`, which is process-global, while the
 * values inside it belong to the realm that made them (ADR 0038). Two realms in one address space
 * therefore write one array and then read each other's values, which presents as
 * `undefined is not a function` at the first import -- measured, before this used fork.
 *
 * A process per realm is also what `node`'s cluster does, for the same reason, and the text
 * segment is shared copy-on-write so the cost is the heaps, which were never shareable anyway. */
int main(int argc, char **argv) {
    long workers;
    int status;
#ifndef _WIN32
    long index = 0;
    long joined;
    pid_t *children;
#endif
    if (setlocale(LC_NUMERIC, "C") == NULL) return 1;
    workers = kiln_cli_worker_count();
#ifdef _WIN32
    (void)workers;
    return kiln_cli_run_realm(argc, argv, 1) == 0 ? 0 : 1;
#else
    if (workers <= 1) return kiln_cli_run_realm(argc, argv, 1) == 0 ? 0 : 1;
    children = (pid_t *)calloc((size_t)(workers - 1), sizeof *children);
    if (children == NULL) return 1;
    for (index = 0; index < workers - 1; ++index) {
        pid_t child = fork();
        if (child == 0) {
            /* A child forked before any realm exists, so it inherits no realm state to trip over.
             * It never writes the result file: one path, one writer. */
            free(children);
            _exit(kiln_cli_run_realm(argc, argv, 0) == 0 ? 0 : 1);
        }
        if (child < 0) break; /* Fewer realms than asked for still serves. None does not. */
        children[index] = child;
    }
    status = kiln_cli_run_realm(argc, argv, 1);
    for (joined = 0; joined < index; ++joined) {
        int child_status = 0;
        if (waitpid(children[joined], &child_status, 0) < 0) continue;
        if (status == 0 && WIFEXITED(child_status) && WEXITSTATUS(child_status) != 0)
            status = WEXITSTATUS(child_status);
    }
    free(children);
    return status == 0 ? 0 : 1;
#endif
}
"##,
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
    copy = (char *)malloc(result->len + 1);
    if (copy == NULL) {{
        proxy->failure_status = KILN_C99_HOST_ERROR;
        return KILN_C99_HOST_ERROR;
    }}
    if (result->len != 0) memcpy(copy, result->ptr, result->len);
    copy[result->len] = '\0';
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
        copy = (char *)malloc(value.len + 1);
        if (copy == NULL) {{
            status = KILN_C99_HOST_ERROR;
            goto cleanup;
        }}
        if (value.len != 0) memcpy(copy, value.ptr, value.len);
        copy[value.len] = '\0';
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
    copy = (char *)malloc(result->len + 1);
    if (copy == NULL) {
        proxy->failure_status = KILN_C99_HOST_ERROR;
        return KILN_C99_HOST_ERROR;
    }
    if (result->len != 0) memcpy(copy, result->ptr, result->len);
    copy[result->len] = '\0';
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
            preserved = (char *)malloc(result->len + 1);
            if (preserved == NULL) {{
                *result = (kiln_value){{ KILN_VALUE_UNDEFINED, 0, {{0}} }};
                status = KILN_C99_HOST_ERROR;
            }} else {{
                if (result->len != 0) memcpy(preserved, result->ptr, result->len);
                preserved[result->len] = '\0';
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

#[cfg(test)]
mod tests;
