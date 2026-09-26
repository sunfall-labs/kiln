//! Cached orchestration for Kiln's C native backend.
//!
//! The backend, runtime, cache, and platform toolchain deliberately live in separate crates.
//! This crate is the narrow coordinator that gives those pieces an ordered build graph:
//! generated module C, application object, reusable runtime object, launcher object, executable.

mod number_provider;
pub mod shared_publication;

use std::collections::{BTreeMap, BTreeSet};
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
    C_ABI_VERSION, C_GENERATOR_VERSION, C99_HOST_MODULE_CREATE_REQUIRE, EmissionPolicy, EmitError,
    RUNTIME_PRELUDE_HEADER_NAME, emit_commonjs_module_with_logical_path_and_policy,
    emit_module_with_logical_path_and_policy, emit_runtime_module_with_logical_path_and_policy,
    emit_with_policy, runtime_prelude_header,
};
use kiln_host_abi::HOST_ABI_VERSION;
pub use kiln_host_abi::HostValue;
use kiln_ir::{ModuleProgram, Program};
use kiln_module_graph::{ModuleGraph, ModuleGraphError, ModuleSource, ResolvedExport};
pub use kiln_native_provider_contract::{
    ProviderCapabilities, ProviderCapability, ProviderVariant,
};
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
use url::Url;

static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);
static MATERIALIZE_COUNTER: AtomicU64 = AtomicU64::new(0);

const RELEASE_PRELUDE_COMPILE_IDENTITY: &[u8] = b"vectorize=off\0slp-vectorize=off";
const MAX_RELEASE_SPECIALIZED_MODULE_INSTRUCTIONS: usize = 512;
const MAX_RELEASE_SPECIALIZED_MODULE_FUNCTIONS: usize = 16;

/// The cache families emitted by this crate. Keeping the family catalogue beside the key
/// construction code makes cache GC follow the producer automatically when a stage is added.
macro_rules! native_artifact_catalogue {
    ($callback:ident) => {
        $callback! {
            sources: [ModuleSource, RuntimeSource, PreludeSource, LauncherSource, GraphLauncherSource, SharedModuleSource, SharedGraphWrapperSource, NumberProvider],
            objects: [ModuleObject, LauncherObject, SharedLibraryObject, SharedPreludeObject, SharedGraphModuleObject, SharedGraphWrapperObject, GraphLauncherObject, SharedLibraryLink, SharedGraphLink, RuntimeLibrary, RuntimePreludeLibrary, ExecutableLink]
        }
    };
}
macro_rules! define_native_artifact_family {
    (sources: [$($source:ident),*], objects: [$($object:ident),*]) => {
        #[derive(Clone, Copy)]
        enum NativeArtifactFamily { $($source,)* $($object,)* }
        impl NativeArtifactFamily {
            const SOURCES: [Self; [$(stringify!($source)),*].len()] = [$(Self::$source,)*];
            const OBJECTS: [Self; [$(stringify!($object)),*].len()] = [$(Self::$object,)*];
        }
    };
}
native_artifact_catalogue!(define_native_artifact_family);

impl NativeArtifactFamily {
    fn artifact_kind(self) -> ArtifactKind {
        match self {
            Self::ModuleSource
            | Self::RuntimeSource
            | Self::PreludeSource
            | Self::LauncherSource
            | Self::GraphLauncherSource
            | Self::SharedModuleSource
            | Self::SharedGraphWrapperSource => ArtifactKind::GeneratedC,
            Self::NumberProvider => ArtifactKind::RuntimeLibrary,
            Self::ModuleObject
            | Self::LauncherObject
            | Self::SharedLibraryObject
            | Self::SharedPreludeObject
            | Self::SharedGraphModuleObject
            | Self::SharedGraphWrapperObject
            | Self::GraphLauncherObject => ArtifactKind::NativeObject,
            Self::SharedLibraryLink | Self::SharedGraphLink => ArtifactKind::LinkedArtifact,
            Self::RuntimeLibrary | Self::RuntimePreludeLibrary => ArtifactKind::RuntimeLibrary,
            Self::ExecutableLink => ArtifactKind::LinkedArtifact,
        }
    }

    fn backend_abi(
        self,
        module_abi: &str,
        runtime_abi: &str,
        embedding_abi: &str,
    ) -> (Backend, String) {
        match self {
            Self::ModuleSource | Self::ModuleObject => (Backend::C, module_abi.into()),
            Self::RuntimeSource => (
                Backend::Custom("native-runtime-source".into()),
                runtime_abi.into(),
            ),
            Self::PreludeSource => (
                Backend::Custom("native-prelude-source".into()),
                runtime_abi.into(),
            ),
            Self::LauncherSource => (
                Backend::Custom("native-launcher-source".into()),
                runtime_abi.into(),
            ),
            Self::LauncherObject => (
                Backend::Custom("native-launcher".into()),
                runtime_abi.into(),
            ),
            Self::GraphLauncherSource => (
                Backend::Custom("native-graph-launcher-source".into()),
                runtime_abi.into(),
            ),
            Self::GraphLauncherObject => (
                Backend::Custom("native-graph-launcher".into()),
                runtime_abi.into(),
            ),
            Self::SharedModuleSource => (
                Backend::Custom("native-shared-library-source".into()),
                embedding_abi.into(),
            ),
            Self::SharedLibraryObject => (
                Backend::Custom("native-shared-library".into()),
                embedding_abi.into(),
            ),
            Self::SharedGraphWrapperSource => (
                Backend::Custom("native-shared-graph-wrapper-source".into()),
                runtime_abi.into(),
            ),
            Self::SharedGraphWrapperObject => (
                Backend::Custom("native-shared-graph-wrapper".into()),
                runtime_abi.into(),
            ),
            Self::NumberProvider => (
                Backend::Custom("rust-native-number-provider".into()),
                native_number_abi_identity(),
            ),
            Self::SharedPreludeObject => (
                Backend::Custom("native-shared-prelude".into()),
                embedding_abi.into(),
            ),
            Self::SharedGraphModuleObject => (
                Backend::Custom("native-shared-graph-module".into()),
                module_abi.into(),
            ),
            Self::SharedLibraryLink => (
                Backend::Custom("native-shared-library".into()),
                embedding_abi.into(),
            ),
            Self::SharedGraphLink => (
                Backend::Custom("native-shared-graph".into()),
                embedding_abi.into(),
            ),
            Self::RuntimeLibrary => (Backend::Custom("native-runtime".into()), runtime_abi.into()),
            Self::RuntimePreludeLibrary => (
                Backend::Custom("native-runtime-prelude".into()),
                runtime_abi.into(),
            ),
            Self::ExecutableLink => (Backend::C, runtime_abi.into()),
        }
    }
    fn compiler_version(self) -> String {
        match self {
            Self::ModuleSource | Self::ModuleObject => {
                format!("kiln-c-abi-{C_ABI_VERSION}-generator-{C_GENERATOR_VERSION}")
            }
            Self::RuntimeSource => {
                format!("kiln-runtime-{NATIVE_RUNTIME_VERSION}")
            }
            Self::PreludeSource => {
                format!("kiln-runtime-prelude-{NATIVE_PRELUDE_VERSION}")
            }
            Self::LauncherSource | Self::LauncherObject => {
                format!("kiln-launcher-{NATIVE_RUNTIME_VERSION}")
            }
            Self::GraphLauncherSource | Self::GraphLauncherObject => {
                format!("kiln-graph-launcher-{NATIVE_RUNTIME_VERSION}")
            }
            Self::SharedModuleSource | Self::SharedLibraryObject => {
                format!("kiln-c-abi-{C_ABI_VERSION}-module-api-{EMBEDDING_MODULE_API_VERSION}")
            }
            Self::SharedGraphWrapperSource | Self::SharedGraphWrapperObject => format!(
                "kiln-shared-graph-wrapper-{NATIVE_RUNTIME_VERSION}-module-api-{EMBEDDING_MODULE_API_VERSION}"
            ),
            Self::NumberProvider => "rust-native-number-provider".into(),
            Self::SharedPreludeObject => "shared-prelude-object".into(),
            Self::SharedGraphModuleObject => "shared-graph-module-object".into(),
            Self::SharedLibraryLink => "shared-library-link".into(),
            Self::SharedGraphLink => "shared-graph-link".into(),
            Self::RuntimeLibrary => format!("kiln-runtime-{NATIVE_RUNTIME_VERSION}"),
            Self::RuntimePreludeLibrary => format!("kiln-runtime-prelude-{NATIVE_PRELUDE_VERSION}"),
            Self::ExecutableLink => {
                format!("kiln-c-abi-{C_ABI_VERSION}-generator-{C_GENERATOR_VERSION}")
            }
        }
    }

    /// Construct the complete cache key for this explicitly selected producer family.
    /// Backend and ABI policy live here so producers cannot accidentally create an identity
    /// which eviction does not know how to retain.
    fn key(
        self,
        compiler: impl Into<String>,
        target: &str,
        profile: CacheProfile,
        options: Digest,
        inputs: impl IntoIterator<Item = Digest>,
    ) -> ArtifactKey {
        let (backend, abi) = self.backend_abi(
            &module_abi_identity(),
            &runtime_abi_identity(),
            &shared_embedding_abi_identity(),
        );
        ArtifactKey::new(
            self.artifact_kind(),
            compiler,
            backend,
            &abi,
            target,
            "generic",
            profile,
            "none",
            options,
            inputs,
        )
    }

    fn source_key(
        self,
        target: &str,
        options: Digest,
        inputs: impl IntoIterator<Item = Digest>,
    ) -> ArtifactKey {
        self.key(
            self.compiler_version(),
            target,
            CacheProfile::Debug,
            options,
            inputs,
        )
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

#[cfg(test)]
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

fn release_module_uses_generic_bodies(module: &ModuleProgram, policy: &EmissionPolicy) -> bool {
    let (instructions, functions) =
        kiln_backend_c::retained_module_size(module, policy.live_module_slots.as_deref());
    functions > MAX_RELEASE_SPECIALIZED_MODULE_FUNCTIONS
        || instructions > MAX_RELEASE_SPECIALIZED_MODULE_INSTRUCTIONS
}

/// Map a native build profile to liveness selection: only release-style
/// builds drop the development arm of the exact two-arm production
/// dispatcher; development, measurement, and interpreter reduction keep
/// both arms live.
pub fn selection_options(profile: &NativeProfile) -> kiln_liveness::SelectionOptions {
    selection_options_with_exclusions(profile, &BTreeSet::new())
}

fn selection_options_with_exclusions(
    profile: &NativeProfile,
    excluded: &BTreeSet<PathBuf>,
) -> kiln_liveness::SelectionOptions {
    kiln_liveness::SelectionOptions {
        production_dispatch: matches!(
            profile,
            NativeProfile::Release | NativeProfile::ReleaseSize | NativeProfile::Profile
        ),
        excluded: excluded.clone(),
    }
}

/// Reduce a linked graph to its closed-world live subset.
///
/// Every remaining import edge is live by construction, so emission and the
/// launcher wire all of them with no dead-slot skipping. See
/// [`ModuleGraph::reduce_to_live`].
fn reduced_graph(
    graph: &ModuleGraph,
    demand: &kiln_module_graph::LiveDemand,
) -> Result<(ModuleGraph, kiln_module_graph::LiveDemand), NativeError> {
    graph.reduce_to_live(demand).map_err(NativeError::Graph)
}

/// Specialize direct ESM exports to the closed-world demand, then let Oxc
/// remove the newly unreferenced local declarations using its purity model.
///
/// The source text predates graph reduction, so compiled imports are reconciled
/// by local binding name with the reduced graph. That both discards already
/// dead bindings and preserves synthetic re-export retargeting. A module whose
/// demanded interface cannot be recovered keeps its original program.
fn dce_release_graph(
    graph: &ModuleGraph,
    demand: &kiln_module_graph::LiveDemand,
) -> (ModuleGraph, kiln_module_graph::LiveDemand) {
    let mut replacements = BTreeMap::new();
    let mut remapped = demand.clone();

    for node in graph.modules() {
        if node.source != ModuleSource::Source {
            continue;
        }
        if node.program.program.source.is_empty() {
            continue;
        }
        let demanded_slots = demand.slots.get(&node.path).cloned().unwrap_or_default();
        let demanded_names = node
            .program
            .exports
            .iter()
            .filter(|export| demanded_slots.contains(&export.local_slot))
            .map(|export| export.exported.clone())
            .collect::<BTreeSet<_>>();
        let Some(path) = node.path.to_str() else {
            continue;
        };
        let hir = frontend_oxc::parse_dce(path, &node.program.program.source, &demanded_names);
        let Ok(mut program) = kiln_ir::compile_module(&hir) else {
            continue;
        };

        let mut imports = Vec::new();
        for mut import in program.imports {
            let Some(linked) = node
                .program
                .imports
                .iter()
                .find(|candidate| candidate.local == import.local)
            else {
                continue;
            };
            import.source.clone_from(&linked.source);
            import.imported.clone_from(&linked.imported);
            imports.push(import);
        }
        program.imports = imports;
        program.dependencies.clone_from(&node.program.dependencies);

        let replacement_slots = program
            .exports
            .iter()
            .filter(|export| demanded_names.contains(&export.exported))
            .map(|export| export.local_slot)
            .collect::<BTreeSet<_>>();
        let replacement_names = program
            .exports
            .iter()
            .filter(|export| replacement_slots.contains(&export.local_slot))
            .map(|export| export.exported.clone())
            .collect::<BTreeSet<_>>();
        if !demanded_names.is_subset(&replacement_names) {
            continue;
        }

        remapped.slots.insert(node.path.clone(), replacement_slots);
        replacements.insert(node.path.clone(), program);
    }

    match graph.replace_programs_and_relink(replacements) {
        Ok(graph) => (graph, remapped),
        // Optimization must not turn a valid linked program into a build
        // failure. An unfamiliar export/re-export shape keeps the already
        // reduced graph and its original slot numbering.
        Err(_) => (graph.clone(), demand.clone()),
    }
}

fn demanded_interface(
    graph: &ModuleGraph,
    demand: &kiln_module_graph::LiveDemand,
) -> BTreeMap<PathBuf, BTreeSet<String>> {
    graph
        .modules()
        .filter(|node| demand.live.contains(&node.path))
        .map(|node| {
            let slots = demand.slots.get(&node.path).cloned().unwrap_or_default();
            let names = node
                .program
                .exports
                .iter()
                .filter(|export| slots.contains(&export.local_slot))
                .map(|export| export.exported.clone())
                .collect();
            (node.path.clone(), names)
        })
        .collect()
}

/// Alternate source-level DCE and graph selection until removing wrappers no longer narrows the
/// demanded module interface. Export slots are deliberately compared by public name: recompiling
/// a module may renumber its private slots even when the observable fixed point is unchanged.
fn dce_release_graph_to_fixpoint(
    mut graph: ModuleGraph,
    mut demand: kiln_module_graph::LiveDemand,
    options: &kiln_liveness::SelectionOptions,
) -> Result<(ModuleGraph, kiln_module_graph::LiveDemand), NativeError> {
    for _ in 0..8 {
        let (specialized, remapped) = dce_release_graph(&graph, &demand);
        let before = demanded_interface(&specialized, &remapped);
        let next =
            kiln_liveness::executable_demand(&specialized, options).map_err(NativeError::Graph)?;
        let (reduced, next) = reduced_graph(&specialized, &next)?;
        let after = demanded_interface(&reduced, &next);
        if before == after && remapped.live == next.live {
            return Ok((reduced, next));
        }
        graph = reduced;
        demand = next;
    }
    Ok((graph, demand))
}

// Select the budget only on a generated-C cache miss. Warm builds must not rerun liveness,
// and the cache payload must remember the actual policy used to compile the emitted body.
fn emit_graph_module_for_profile(
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &EmissionPolicy,
    profile: NativeProfile,
) -> Result<kiln_backend_c::CModule, NativeError> {
    let mut policy = policy.clone();
    if profile == NativeProfile::Release {
        policy.generic_only = release_module_uses_generic_bodies(&node.program, &policy);
    } else if profile == NativeProfile::ReleaseSize {
        policy.generic_only = true;
        policy.omit_source = true;
    }
    policy.compact = profile != NativeProfile::Profile;
    emit_native_graph_module_with_policy(node, logical_path, &policy)
}

fn configure_release_prelude_compile(options: &mut CompileOptions) {
    // Vectorizing this branch-heavy runtime unit accounts for seconds of cold compile time while
    // its generated numeric loops live in application units. Leave scalar inlining enabled: it
    // materially improves collection-heavy runtime workloads.
    options
        .extra_args
        .extend(["-fno-vectorize".into(), "-fno-slp-vectorize".into()]);
}

fn configure_release_module_compile(options: &mut CompileOptions, generic_only: bool) {
    if generic_only {
        // Generic bodies are deliberately used for the largest modules, where optimizing them
        // makes small real-world bundles materially slower despite reducing object size. Keep
        // this path at O0 by default, or respect KILN_GENERIC_OPT (e.g. 1, 2, 3, z).
        let opt = std::env::var("KILN_GENERIC_OPT").unwrap_or_else(|_| "0".into());
        options.extra_args.push(format!("-O{opt}").into());
        // Clang treats an explicit O0 as its Debug configuration and otherwise injects UBSan
        // calls that the release link intentionally does not carry.
        options.extra_args.push("-fno-sanitize=undefined".into());
        // Closed-world liveness already removed unreachable bodies. Giving every remaining
        // helper its own ELF/Mach-O section creates tens of thousands of section records only
        // for the linker to retain them; one code/data section is cheaper to emit and link.
        options.extra_args.push("-fno-function-sections".into());
        options.extra_args.push("-fno-data-sections".into());
    } else {
        options.extra_args.push("-O2".into());
    }
}

fn configure_release_size_module_compile(options: &mut CompileOptions) {
    options.extra_args.push("-fno-sanitize=undefined".into());
    options.extra_args.push("-fno-function-sections".into());
    options.extra_args.push("-fno-data-sections".into());
    options.extra_args.push("-fvisibility=hidden".into());
}

fn release_module_compile_identity(generic_only: bool) -> String {
    if generic_only {
        if let Ok(opt) = std::env::var("KILN_GENERIC_OPT") {
            format!("generic-o{opt}-nosan-nosections")
        } else {
            "generic-o0-nosan-nosections".into()
        }
    } else {
        "o2".into()
    }
}

fn release_size_module_compile_identity() -> &'static str {
    "oz-nosan-nosections-hidden"
}

fn prelude_object_options(base: Digest, profile: NativeProfile) -> Digest {
    if !matches!(profile, NativeProfile::Release | NativeProfile::ReleaseSize) {
        return base;
    }
    let mut identity =
        Vec::with_capacity(base.as_bytes().len() + RELEASE_PRELUDE_COMPILE_IDENTITY.len());
    identity.extend_from_slice(base.as_bytes());
    identity.extend_from_slice(RELEASE_PRELUDE_COMPILE_IDENTITY);
    Digest::hash(&identity)
}

fn pgo_options_fingerprint(options: &NativeBuildOptions) -> String {
    if let Some(dir) = &options.pgo_generate {
        format!("\0pgo-gen:{}", dir.display())
    } else if let Some(path) = &options.pgo_use {
        let digest = std::fs::read(path)
            .map(|b| Digest::hash(&b))
            .unwrap_or_default();
        format!("\0pgo-use:{}:{digest}", path.display())
    } else {
        String::new()
    }
}

fn executable_link_options_digest(profile: NativeProfile, options: &NativeBuildOptions) -> Digest {
    let static_opt = if options.static_link { "\0static" } else { "" };
    let pgo_opt = pgo_options_fingerprint(options);
    Digest::hash(
        format!(
            "{}\0system-math\0system-entropy-v1{static_opt}{pgo_opt}",
            profile.as_str()
        )
        .as_bytes(),
    )
}

/// Prelude object options folded with a dispatch-group mask.
///
/// A full mask returns today's identity byte-for-byte: programs that retain
/// every group reuse every cached prelude object. Any dropped group extends
/// the identity so masked preludes never alias unmasked ones.
fn prelude_object_options_masked(base: Digest, profile: NativeProfile, mask: u32) -> Digest {
    use kiln_backend_c::facts::{CORE_METHOD_GROUPS, PRUNABLE_METHOD_GROUPS};
    let unmasked = prelude_object_options(base, profile);
    if mask == CORE_METHOD_GROUPS | PRUNABLE_METHOD_GROUPS {
        return unmasked;
    }
    let mut identity = Vec::with_capacity(unmasked.as_bytes().len() + std::mem::size_of::<u32>());
    identity.extend_from_slice(unmasked.as_bytes());
    identity.extend_from_slice(&mask.to_le_bytes());
    Digest::hash(&identity)
}

/// `-D` flags compiling dispatch groups out of the prelude.
///
/// One `-DKILN_RETAIN_GROUP_<name>=0` per dropped prunable group; a full mask
/// yields no flags, so fully retained builds compile exactly today's prelude.
fn prelude_group_defines(mask: u32) -> Vec<std::ffi::OsString> {
    use kiln_backend_c::facts::method_groups as groups;
    const FLAGS: [(u32, &str); 18] = [
        (groups::URL, "KILN_RETAIN_GROUP_URL"),
        (groups::TEMPORAL, "KILN_RETAIN_GROUP_TEMPORAL"),
        (groups::TEMPORAL_FROM, "KILN_RETAIN_GROUP_TEMPORAL_FROM"),
        (
            groups::TEMPORAL_COMPARE,
            "KILN_RETAIN_GROUP_TEMPORAL_COMPARE",
        ),
        (
            groups::TEMPORAL_NOW_READ,
            "KILN_RETAIN_GROUP_TEMPORAL_NOW_READ",
        ),
        (groups::TEMPORAL_OP, "KILN_RETAIN_GROUP_TEMPORAL_OP"),
        (groups::DATE_SETTER, "KILN_RETAIN_GROUP_DATE_SETTER"),
        (groups::MATH_NUMBER, "KILN_RETAIN_GROUP_MATH_NUMBER"),
        (groups::NODE_EVENT, "KILN_RETAIN_GROUP_NODE_EVENT"),
        (groups::NODE_NET, "KILN_RETAIN_GROUP_NODE_NET"),
        (groups::NODE_SERVER, "KILN_RETAIN_GROUP_NODE_SERVER"),
        (groups::NODE_HTTP, "KILN_RETAIN_GROUP_NODE_HTTP"),
        (groups::NODE_RESPONSE, "KILN_RETAIN_GROUP_NODE_RESPONSE"),
        (groups::BUFFER, "KILN_RETAIN_GROUP_BUFFER"),
        (groups::NODE_UTIL, "KILN_RETAIN_GROUP_NODE_UTIL"),
        (
            groups::NODE_DIAGNOSTICS,
            "KILN_RETAIN_GROUP_NODE_DIAGNOSTICS",
        ),
        (groups::NODE_STREAM, "KILN_RETAIN_GROUP_NODE_STREAM"),
        (groups::NODE_PATH, "KILN_RETAIN_GROUP_NODE_PATH"),
    ];
    FLAGS
        .iter()
        .filter(|(bit, _)| mask & bit == 0)
        .map(|(_, name)| std::ffi::OsString::from(format!("-D{name}=0")))
        .collect()
}

fn prelude_header_digest() -> Digest {
    Digest::hash(runtime_prelude_header().as_bytes())
}

fn materialize_prelude_header(root: &Path) -> Result<(), NativeError> {
    materialize(
        &root.join(RUNTIME_PRELUDE_HEADER_NAME),
        runtime_prelude_header().as_bytes(),
        false,
    )
}
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
    assert!(PROVIDER_SCHEMA_VERSION == 4);
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
    number_providers: [OnceLock<Result<NumberProvider, String>>; ProviderVariant::COUNT],
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

fn resolve_type_profile(
    options: &NativeBuildOptions,
    entry_path: Option<&std::path::Path>,
) -> Option<kiln_backend_c::TypeProfile> {
    let candidate = if let Some(path) = &options.realm.type_profile_path {
        Some(path.clone())
    } else if let Some(entry) = entry_path {
        let p = entry.with_extension("typeprofile");
        if p.is_file() {
            Some(p)
        } else {
            let out_p = options.output.with_extension("typeprofile");
            if out_p.is_file() {
                Some(out_p)
            } else {
                std::env::var_os("KILN_TYPE_PROFILE_FILE").map(PathBuf::from)
            }
        }
    } else {
        let out_p = options.output.with_extension("typeprofile");
        if out_p.is_file() {
            Some(out_p)
        } else {
            std::env::var_os("KILN_TYPE_PROFILE_FILE").map(PathBuf::from)
        }
    };
    if let Some(path) = candidate
        && let Ok(content) = std::fs::read_to_string(&path)
        && let Ok(prof) = kiln_backend_c::TypeProfile::parse_json(&content)
    {
        return Some(prof);
    }
    None
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
            number_providers: [const { OnceLock::new() }; ProviderVariant::COUNT],
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
        let mut identities = BTreeSet::new();

        // Sources are debug identities; each provider variant is a release archive. Resolve both
        // through the same cached loader used by producers so the catalogue never interprets the
        // provider's opaque structured identity or rebuilds a variant per lookup.
        for family in NativeArtifactFamily::SOURCES {
            if matches!(family, NativeArtifactFamily::NumberProvider) {
                continue;
            }
            identities.insert(
                family
                    .key(
                        family.compiler_version(),
                        &target,
                        CacheProfile::Debug,
                        Digest::hash(b"identity representative"),
                        [],
                    )
                    .identity_digest(),
            );
        }
        for profile in [
            NativeProfile::Dev,
            NativeProfile::Check,
            NativeProfile::Release,
            NativeProfile::ReleaseSize,
            NativeProfile::Profile,
        ] {
            let profile = profile.cache();
            for family in NativeArtifactFamily::OBJECTS {
                identities.insert(
                    family
                        .key(
                            compiler.clone(),
                            &target,
                            profile.clone(),
                            Digest::hash(b"identity representative"),
                            [],
                        )
                        .identity_digest(),
                );
            }
        }
        let provider_identities = number_provider::producer_identities(tool).map_err(|error| {
            NativeError::NumberProvider {
                message: error.to_string(),
            }
        })?;
        for (_variant, provider_identity) in provider_identities {
            identities.insert(
                NativeArtifactFamily::NumberProvider
                    .key(
                        provider_identity,
                        &target,
                        CacheProfile::Release,
                        Digest::hash(b"identity representative"),
                        [],
                    )
                    .identity_digest(),
            );
        }

        Ok(identities)
    }

    fn number_provider_variant(
        &self,
        variant: ProviderVariant,
    ) -> Result<NumberProvider, NativeError> {
        self.number_providers[variant.index()]
            .get_or_init(|| {
                number_provider::discover_for(self.toolchain.identity(), variant.capabilities())
                    .map_err(|error| error.to_string())
            })
            .clone()
            .map_err(|message| NativeError::NumberProvider { message })
    }

    fn cached_number_provider(
        &self,
        target: &str,
    ) -> Result<(Arc<[u8]>, Digest, bool), NativeError> {
        let (archive, digest, hit, _) =
            self.cached_number_provider_for(target, ProviderCapabilities::FULL)?;
        Ok((archive, digest, hit))
    }

    /// Resolve and cache the smallest provider variant covering `required`.
    fn cached_number_provider_for(
        &self,
        target: &str,
        required: ProviderCapabilities,
    ) -> Result<(Arc<[u8]>, Digest, bool, ProviderCapabilities), NativeError> {
        let variant = ProviderVariant::smallest_covering(required).unwrap_or(ProviderVariant::Full);
        let provider = self.number_provider_variant(variant)?;
        let provider_key = NativeArtifactFamily::NumberProvider.key(
            &provider.identity,
            target,
            CacheProfile::Release,
            Digest::hash(
                format!(
                    "schema={PROVIDER_SCHEMA_VERSION};abi={PROVIDER_ABI_VERSION};target={};pic=true;linkage=static;variant={};capabilities={:?}",
                    provider.rust_target,
                    provider.variant.as_str(),
                    provider.capabilities.names(),
                )
                .as_bytes(),
            ),
            [provider.archive_digest],
        );
        let capabilities = provider.capabilities;
        match self.lookup_digest(&provider_key)? {
            Some((digest, _size)) => {
                debug_assert_eq!(digest, provider.archive_digest);
                Ok((Arc::clone(&provider.archive), digest, true, capabilities))
            }
            None => {
                let stored = self.cache.put(&provider_key, &provider.archive)?;
                Ok((
                    Arc::clone(&provider.archive),
                    stored.digest,
                    false,
                    capabilities,
                ))
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
        let mut emission = options.realm.emission_executable();
        emission.compact = options.profile != NativeProfile::Profile;
        if emission.compile_regex.is_none() {
            emission.compile_regex = Some(matches!(
                options.profile,
                NativeProfile::Release | NativeProfile::ReleaseSize | NativeProfile::Profile
            ));
        }
        emission.generic_only = matches!(
            options.profile,
            NativeProfile::Dev | NativeProfile::Check | NativeProfile::ReleaseSize
        );
        if options.profile == NativeProfile::ReleaseSize {
            emission.omit_source = true;
        }
        if options.profile == NativeProfile::Profile {
            emission.type_profile_record = true;
        }
        if let Some(profile) = resolve_type_profile(options, None) {
            emission.type_profile = Some(std::sync::Arc::new(profile));
        }
        let module = emit_with_policy(program, emission.clone())?;
        let launcher = launcher_source(&module.symbol)?;
        let emit_elapsed = emit_started.elapsed();
        let runtime = runtime_source();

        let workspace = WorkDirectory::create()?;
        let paths = BuildPaths::new(workspace.path());
        let identity = self.toolchain.identity().clone();
        let profile = options.profile;
        let target = target_identity(&identity);
        let compiler = compiler_identity(&identity);
        // The single-module API sees only bytecode, not the import table, so a static
        // `node:http` use is invisible here: stay on the full archive.
        let required_provider_capabilities = ProviderCapabilities::FULL;
        let (number_provider, number_provider_digest, _, provider_capabilities) =
            self.cached_number_provider_for(&target, required_provider_capabilities)?;

        let module_source_key = NativeArtifactFamily::ModuleSource.source_key(
            &target,
            Digest::hash(b"deterministic-c11"),
            [Digest::hash(module.source.as_bytes())],
        );
        let (module_source, module_source_digest, generated_c_hit) =
            self.cached_payload(&module_source_key, module.source.as_bytes())?;

        let runtime_source_key = NativeArtifactFamily::RuntimeSource.source_key(
            &target,
            Digest::hash(b"portable-runtime-c11"),
            [Digest::hash(runtime.as_bytes())],
        );
        let (runtime_bytes, runtime_source_digest, runtime_generated_c_cache_hit) =
            self.cached_payload(&runtime_source_key, runtime.as_bytes())?;

        let prelude_source = kiln_backend_c::runtime_prelude_source_with_policy(&emission);
        let prelude_source_key = NativeArtifactFamily::PreludeSource.source_key(
            &target,
            Digest::hash(b"portable-prelude-c11"),
            [Digest::hash(prelude_source.as_bytes())],
        );
        let (prelude_bytes, prelude_source_digest, _) =
            self.cached_payload(&prelude_source_key, prelude_source.as_bytes())?;

        let launcher_source_key = NativeArtifactFamily::LauncherSource.source_key(
            &target,
            Digest::hash(b"portable-launcher-c11"),
            [Digest::hash(launcher.as_bytes())],
        );
        let (launcher_bytes, launcher_source_digest, launcher_generated_c_cache_hit) =
            self.cached_payload(&launcher_source_key, launcher.as_bytes())?;

        let march_opt = if options.march_native {
            "\0march-native"
        } else {
            ""
        };
        let pgo_opt = pgo_options_fingerprint(options);
        let module_opt = if profile == NativeProfile::Release {
            release_module_compile_identity(emission.generic_only)
        } else if profile == NativeProfile::ReleaseSize {
            release_size_module_compile_identity().to_string()
        } else {
            "profile-default".to_string()
        };
        let object_options = Digest::hash(
            format!(
                "c11\0{}\0{}\0generator-{C_GENERATOR_VERSION}{march_opt}{pgo_opt}\0module-opt={module_opt}",
                identity.kind,
                profile.as_str()
            )
            .as_bytes(),
        );
        let application_key = NativeArtifactFamily::ModuleObject.key(
            &compiler,
            &target,
            profile.cache(),
            object_options,
            [module_source_digest, prelude_header_digest()],
        );
        let prelude_key = NativeArtifactFamily::RuntimePreludeLibrary.key(
            &compiler,
            &target,
            profile.cache(),
            prelude_object_options(object_options, profile),
            [prelude_source_digest],
        );
        let runtime_key = NativeArtifactFamily::RuntimeLibrary.key(
            &compiler,
            &target,
            profile.cache(),
            object_options,
            [runtime_source_digest],
        );
        let launcher_key = NativeArtifactFamily::LauncherObject.key(
            &compiler,
            &target,
            profile.cache(),
            object_options,
            [launcher_source_digest],
        );

        let link_options_digest = executable_link_options_digest(profile, options);
        if let Some(app_info) = self.lookup_digest(&application_key)?
            && let Some(prelude_info) = self.lookup_digest(&prelude_key)?
            && let Some(runtime_info) = self.lookup_digest(&runtime_key)?
            && let Some(launcher_info) = self.lookup_digest(&launcher_key)?
        {
            let candidate_executable_key = NativeArtifactFamily::ExecutableLink.key(
                &compiler,
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
                        prelude_object: true,
                        runtime_object: true,
                        launcher_object: true,
                        executable: true,
                    },
                    sizes: ArtifactSizes {
                        generated_c: module_source.len() as u64,
                        application_object: app_info.1,
                        prelude_object: prelude_info.1,
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
                    provider_capabilities,
                    required_provider_capabilities,
                });
            }
        }

        materialize_prelude_header(workspace.path())?;
        let compile_options = CompileOptions {
            profile: profile.toolchain(),
            march_native: options.march_native,
            pgo_generate: options.pgo_generate.clone(),
            pgo_use: options.pgo_use.clone(),
            ..CompileOptions::default()
        };
        let mut app_compile_options = compile_options.clone();
        if profile == NativeProfile::Release {
            configure_release_module_compile(&mut app_compile_options, emission.generic_only);
        } else if profile == NativeProfile::ReleaseSize {
            configure_release_size_module_compile(&mut app_compile_options);
        }
        let mut prelude_compile_options = compile_options.clone();
        if matches!(profile, NativeProfile::Release | NativeProfile::ReleaseSize) {
            configure_release_prelude_compile(&mut prelude_compile_options);
        }
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
                                        &app_compile_options,
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
                                        &prelude_compile_options,
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

        let link_options_digest = executable_link_options_digest(profile, options);
        let executable_key = NativeArtifactFamily::ExecutableLink.key(
            &compiler,
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
                        static_link: options.static_link,
                        pgo_generate: options.pgo_generate.clone(),
                        pgo_use: options.pgo_use.clone(),
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
                prelude_object: prelude_hit,
                runtime_object: runtime_hit,
                launcher_object: launcher_hit,
                executable: executable_hit,
            },
            sizes: ArtifactSizes {
                generated_c: module_source.len() as u64,
                application_object: application.len() as u64,
                prelude_object: prelude_object.len() as u64,
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
            provider_capabilities,
            required_provider_capabilities,
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
        let mut emission = options.realm.emission();
        emission.compact = options.profile != NativeProfile::Profile;
        if emission.compile_regex.is_none() {
            emission.compile_regex = Some(matches!(
                options.profile,
                NativeProfile::Release | NativeProfile::ReleaseSize | NativeProfile::Profile
            ));
        }
        emission.generic_only = matches!(
            options.profile,
            NativeProfile::Dev | NativeProfile::Check | NativeProfile::ReleaseSize
        );
        if options.profile == NativeProfile::ReleaseSize {
            emission.omit_source = true;
        }
        let module = emit_with_policy(program, emission.clone())?;
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
        let embedding_abi = shared_embedding_abi_identity();
        let target = target_identity(&identity);
        let compiler = compiler_identity(&identity);
        let (number_provider, number_provider_digest, _) = self.cached_number_provider(&target)?;
        // Keep the plain generated unit on the same cache identity as executable builds.  The
        // shared-only adapter is cached as a second generated-C artifact, so building a DSO does
        // not poison the reusable non-PIC module source cache.
        let module_source_key = NativeArtifactFamily::ModuleSource.source_key(
            &target,
            Digest::hash(b"deterministic-c11"),
            [Digest::hash(module.source.as_bytes())],
        );
        let (base_source, base_source_digest, _) =
            self.cached_payload(&module_source_key, module.source.as_bytes())?;
        let shared_source_key = NativeArtifactFamily::SharedModuleSource.source_key(
            &target,
            Digest::hash(b"deterministic-c11-owned-result-v2"),
            [base_source_digest, Digest::hash(module_wrapper.as_bytes())],
        );
        let mut shared_module_source = base_source;
        shared_module_source.push(b'\n');
        shared_module_source.extend_from_slice(module_wrapper.as_bytes());
        let (module_source, module_source_digest, generated_c_hit) =
            self.cached_payload(&shared_source_key, &shared_module_source)?;
        let march_opt = if options.march_native {
            "\0march-native"
        } else {
            ""
        };
        let object_options = Digest::hash(
            format!(
                "shared-library\0pic\0c11\0{}\0{}\0generator-{C_GENERATOR_VERSION}{march_opt}",
                identity.kind,
                profile.as_str()
            )
            .as_bytes(),
        );
        let object_key = NativeArtifactFamily::SharedLibraryObject.key(
            &compiler,
            &target,
            profile.cache(),
            object_options,
            [module_source_digest, prelude_header_digest()],
        );
        let prelude_source = kiln_backend_c::runtime_prelude_source_with_policy(&emission);
        let prelude_source_key = NativeArtifactFamily::PreludeSource.source_key(
            &target,
            Digest::hash(b"portable-prelude-c11"),
            [Digest::hash(prelude_source.as_bytes())],
        );
        let (prelude_bytes, prelude_source_digest, _) =
            self.cached_payload(&prelude_source_key, prelude_source.as_bytes())?;
        let prelude_key = NativeArtifactFamily::SharedPreludeObject.key(
            &compiler,
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
            let library_key = NativeArtifactFamily::SharedLibraryLink.key(
                &compiler,
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

        materialize_prelude_header(workspace.path())?;
        let compile_options = CompileOptions {
            profile: profile.toolchain(),
            position_independent: true,
            march_native: options.march_native,
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
        let library_key = NativeArtifactFamily::SharedLibraryLink.key(
            &compiler,
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
        let native_plan = native_graph_plan(graph)?;
        // Shared libraries embed for unknown hosts: development selection
        // keeps both arms of the production dispatcher and every live edge,
        // while dead modules are pruned like any other executable.
        let demand = kiln_liveness::executable_demand(
            graph,
            &selection_options_with_exclusions(&NativeProfile::Dev, &native_plan.excluded),
        )
        .map_err(NativeError::Graph)?;
        let (reduced, demand) = reduced_graph(graph, &demand)?;
        let (reduced, demand) = if matches!(
            options.profile,
            NativeProfile::Release | NativeProfile::ReleaseSize
        ) {
            dce_release_graph_to_fixpoint(
                reduced,
                demand,
                &kiln_liveness::SelectionOptions::development(),
            )?
        } else {
            (reduced, demand)
        };
        let graph = &reduced;
        let mut emission = options.realm.emission();
        emission.compact = options.profile != NativeProfile::Profile;
        if emission.compile_regex.is_none() {
            emission.compile_regex = Some(matches!(
                options.profile,
                NativeProfile::Release | NativeProfile::ReleaseSize | NativeProfile::Profile
            ));
        }
        emission.generic_only = matches!(
            options.profile,
            NativeProfile::Dev | NativeProfile::Check | NativeProfile::ReleaseSize
        );
        if options.profile == NativeProfile::ReleaseSize {
            emission.omit_source = true;
        }
        // Shared graph modules link against one prelude object; keep runtime definitions out of
        // every module object so exported graph helpers have a single authoritative owner.
        emission.decoupled_prelude = true;
        let total_started = Instant::now();
        let emit_started = Instant::now();
        let identity = self.toolchain.identity().clone();
        let target = target_identity(&identity);
        let nodes = graph
            .modules()
            .filter(|node| !native_plan.excluded.contains(&node.path))
            .collect::<Vec<_>>();
        let emitted_modules = emit_indexed(nodes.len(), "shared graph module emission", |index| {
            let node = nodes[index];
            let logical_path = logical_module_path(graph.project_root(), &node.path)?;
            let mut module_emission = emission.clone();
            if matches!(node.source, ModuleSource::Source | ModuleSource::CommonJs) {
                module_emission.live_module_slots = Some(Arc::new(
                    demand.slots.get(&node.path).cloned().unwrap_or_default(),
                ));
            }
            cached_graph_module(
                &self.cache,
                &target,
                node,
                &logical_path,
                &module_emission,
                || {
                    emit_graph_module_for_profile(
                        node,
                        &logical_path,
                        &module_emission,
                        options.profile,
                    )
                },
            )
        })?;
        let mut modules = Vec::with_capacity(nodes.len());
        let mut generated_c_hits = Vec::with_capacity(nodes.len());
        let mut symbols = std::collections::HashSet::with_capacity(graph.module_count());
        for (node, (emitted, generated_c_hit)) in nodes.into_iter().zip(emitted_modules) {
            if !symbols.insert(emitted.symbol.clone()) {
                return Err(NativeError::SymbolCollision {
                    symbol: emitted.symbol,
                });
            }
            generated_c_hits.push(generated_c_hit);
            modules.push((node, emitted));
        }
        if modules.is_empty() {
            return Err(NativeError::Graph(
                ModuleGraphError::InternalMissingModule {
                    path: graph.entry().to_path_buf(),
                },
            ));
        }
        let topology = graph_shared_topology_digest(
            graph,
            &modules,
            Some(&native_plan.failures),
            Some(&native_plan.runtime_failures),
        );
        let wrapper_symbol = format!(
            "kiln_shared_graph_{}",
            graph_shared_public_digest(topology, &modules)?.to_hex()
        );
        let wrapper = graph_shared_wrapper_source_with_failures(
            graph,
            &modules,
            &wrapper_symbol,
            &native_plan.failures,
            &native_plan.runtime_failures,
        )?;
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
        let mut symbol_list = Vec::new();
        let mut module_sources = Vec::new();
        let mut module_generic_only = Vec::new();
        for ((_, module), &hit) in modules.iter().zip(&generated_c_hits) {
            if module.chunks.is_empty() {
                symbol_list.push(module.symbol.clone());
                module_sources.push((
                    module.source.as_bytes().to_vec(),
                    Digest::hash(module.source.as_bytes()),
                    hit,
                ));
                module_generic_only.push(module.generic_only);
            } else {
                for (chunk_idx, chunk_source) in module.chunks.iter().enumerate() {
                    symbol_list.push(format!("{}_chunk_{chunk_idx}", module.symbol));
                    module_sources.push((
                        chunk_source.as_bytes().to_vec(),
                        Digest::hash(chunk_source.as_bytes()),
                        hit,
                    ));
                    module_generic_only.push(module.generic_only);
                }
            }
        }
        let paths = SharedGraphLibraryPaths::new(workspace.path(), &symbol_list);
        let profile = options.profile;
        let embedding_abi = runtime_abi_identity();
        let compiler = compiler_identity(&identity);
        let (number_provider, number_provider_digest, _) = self.cached_number_provider(&target)?;
        let wrapper_source_key = NativeArtifactFamily::SharedGraphWrapperSource.source_key(
            &target,
            Digest::hash(b"deterministic-c11-shared-graph-wrapper-owned-result-v2"),
            [Digest::hash(wrapper.as_bytes()), topology],
        );
        let (wrapper_bytes, wrapper_source_digest, wrapper_generated_c_hit) =
            self.cached_payload(&wrapper_source_key, wrapper.as_bytes())?;

        let march_opt = if options.march_native {
            "\0march-native"
        } else {
            ""
        };
        let module_object_options = module_generic_only
            .iter()
            .map(|&generic_only| {
                Digest::hash(
                    format!(
                        "shared-graph\0pic\0c11\0visibility-hidden\0{}\0{}{march_opt}\0module-opt={}",
                        identity.kind,
                        profile.as_str(),
                        if profile == NativeProfile::Release {
                            release_module_compile_identity(generic_only)
                        } else if profile == NativeProfile::ReleaseSize {
                            release_size_module_compile_identity().to_string()
                        } else {
                            "profile-default".to_string()
                        }
                    )
                    .as_bytes(),
                )
            })
            .collect::<Vec<_>>();
        let wrapper_object_options = Digest::hash(
            format!(
                "shared-graph\0pic\0c11\0{}\0{}{march_opt}",
                identity.kind,
                profile.as_str()
            )
            .as_bytes(),
        );
        // Graph module symbols are an internal link protocol. Different shared libraries can
        // contain the same logical path with different bytecode, so exporting those symbols lets
        // a dynamic loader interpose one library's module into another library's wrapper. Keep
        // the six-argument entries externally linkable inside this DSO but hidden outside it.
        let module_compile_options = module_generic_only
            .iter()
            .map(|&generic_only| {
                let mut compile = CompileOptions {
                    profile: profile.toolchain(),
                    position_independent: true,
                    march_native: options.march_native,
                    ..CompileOptions::default()
                };
                compile.extra_args.push("-fvisibility=hidden".into());
                if profile == NativeProfile::Release {
                    configure_release_module_compile(&mut compile, generic_only);
                } else if profile == NativeProfile::ReleaseSize {
                    configure_release_size_module_compile(&mut compile);
                }
                compile
            })
            .collect::<Vec<_>>();
        let wrapper_compile_options = CompileOptions {
            profile: profile.toolchain(),
            position_independent: true,
            march_native: options.march_native,
            ..CompileOptions::default()
        };
        let mut module_keys = Vec::with_capacity(module_sources.len());
        for index in 0..module_sources.len() {
            let (_, source_digest, _) = &module_sources[index];
            module_keys.push(NativeArtifactFamily::SharedGraphModuleObject.key(
                &compiler,
                &target,
                profile.cache(),
                module_object_options[index],
                [*source_digest, prelude_header_digest()],
            ));
        }
        let wrapper_object_key = NativeArtifactFamily::SharedGraphWrapperObject.key(
            &compiler,
            &target,
            profile.cache(),
            wrapper_object_options,
            [wrapper_source_digest, topology],
        );
        let prelude_source = kiln_backend_c::runtime_prelude_source_with_policy(&emission);
        let prelude_source_key = NativeArtifactFamily::PreludeSource.source_key(
            &target,
            Digest::hash(b"portable-prelude-c11"),
            [Digest::hash(prelude_source.as_bytes())],
        );
        let (prelude_bytes, prelude_source_digest, _) =
            self.cached_payload(&prelude_source_key, prelude_source.as_bytes())?;
        let shared_prelude_object_options = Digest::hash(
            format!(
                "shared-library\0pic\0c11\0{}\0{}\0generator-{C_GENERATOR_VERSION}{march_opt}",
                identity.kind,
                profile.as_str()
            )
            .as_bytes(),
        );
        let prelude_key = NativeArtifactFamily::SharedPreludeObject.key(
            &compiler,
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
            let library_key = NativeArtifactFamily::SharedGraphLink.key(
                &compiler,
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

        materialize_prelude_header(workspace.path())?;
        let compile_started = Instant::now();
        let (module_objects, other_objects) = rayon::join(
            || {
                let module_keys_ref = &module_keys;
                let module_sources_ref = &module_sources;
                let module_c_ref = &paths.module_c;
                let module_o_ref = &paths.module_o;
                let module_compile_options_ref = &module_compile_options;
                let sizes: Vec<_> = module_sources_ref
                    .iter()
                    .map(|(source, _, _)| source.len())
                    .collect();
                compile_largest_first(&sizes, "module compile", move |index| {
                    self.object(
                        &module_keys_ref[index],
                        &module_sources_ref[index].0,
                        &module_c_ref[index],
                        &module_o_ref[index],
                        &module_compile_options_ref[index],
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
        let library_key = NativeArtifactFamily::SharedGraphLink.key(
            &compiler,
            &target,
            profile.cache(),
            link_options_digest,
            link_inputs.iter().copied(),
        );
        let (library, library_hit, link_elapsed) = match self.lookup(&library_key)? {
            Lookup::Hit(artifact) => (artifact.bytes, true, Duration::ZERO),
            Lookup::Miss => {
                let mut objects = Vec::with_capacity(module_objects.len() + 2);
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
        let native_plan = native_graph_plan(graph)?;
        let total_started = Instant::now();
        let emit_started = Instant::now();
        let mut emission = options.realm.emission_executable();
        emission.compact = options.profile != NativeProfile::Profile;
        if emission.compile_regex.is_none() {
            emission.compile_regex = Some(matches!(
                options.profile,
                NativeProfile::Release | NativeProfile::ReleaseSize | NativeProfile::Profile
            ));
        }
        emission.generic_only = matches!(
            options.profile,
            NativeProfile::Dev | NativeProfile::Check | NativeProfile::ReleaseSize
        );
        if options.profile == NativeProfile::ReleaseSize {
            emission.omit_source = true;
        }
        if options.profile == NativeProfile::Profile {
            emission.type_profile_record = true;
        }
        let entry_module_path = graph.module(graph.entry()).map(|m| m.path.as_path());
        if let Some(profile) = resolve_type_profile(options, entry_module_path) {
            emission.type_profile = Some(std::sync::Arc::new(profile));
        }
        let identity = self.toolchain.identity().clone();
        let target = target_identity(&identity);
        let selection = selection_options_with_exclusions(&options.profile, &native_plan.excluded);
        let selected = kiln_liveness::select_executable_modules(graph, &selection)
            .map_err(NativeError::Graph)?;
        // One shaken graph feeds emission: reduce to the live subset first,
        // so the emitter and launcher below only ever see live modules and
        // live edges. With everything live this is the identity transform.
        let demand = selected.demand();
        let (reduced, demand) = reduced_graph(graph, &demand)?;
        let (reduced, demand) = if matches!(
            options.profile,
            NativeProfile::Release | NativeProfile::ReleaseSize
        ) {
            dce_release_graph_to_fixpoint(reduced, demand, &selection)?
        } else {
            (reduced, demand)
        };
        let graph = &reduced;
        // Dispatch-group mask for the prelude: same closed-world selection,
        // so the `-D` flags below only ever remove what nothing retained can
        // reach. Computed once; the explain path recomputes it out-of-band.
        let members =
            kiln_backend_c::facts::analyze_demanded_members(graph, &demand.live, &demand.slots);
        let group_mask = kiln_liveness::retained_method_groups(graph, &demand, &members);
        let nodes: Vec<&kiln_module_graph::ModuleNode> = graph.modules().collect();
        let emitted_modules = emit_indexed(nodes.len(), "graph module emission", |index| {
            let node = nodes[index];
            let logical_path = logical_module_path(graph.project_root(), &node.path)?;
            let mut module_emission = emission.clone();
            if matches!(node.source, ModuleSource::Source | ModuleSource::CommonJs) {
                module_emission.live_module_slots = Some(Arc::new(
                    demand.slots.get(&node.path).cloned().unwrap_or_default(),
                ));
            }
            let (emitted, generated_c_hit) = cached_graph_module(
                &self.cache,
                &target,
                node,
                &logical_path,
                &module_emission,
                || {
                    emit_graph_module_for_profile(
                        node,
                        &logical_path,
                        &module_emission,
                        options.profile,
                    )
                },
            )?;
            Ok((logical_path, emitted, generated_c_hit))
        })?;
        let mut modules = Vec::with_capacity(nodes.len());
        let mut logical_paths = Vec::with_capacity(nodes.len());
        let mut generated_c_hits = Vec::with_capacity(nodes.len());
        let mut symbols = std::collections::HashSet::with_capacity(graph.module_count());
        for (node, (logical_path, emitted, generated_c_hit)) in
            nodes.into_iter().zip(emitted_modules)
        {
            if !symbols.insert(emitted.symbol.clone()) {
                return Err(NativeError::SymbolCollision {
                    symbol: emitted.symbol,
                });
            }
            logical_paths.push(logical_path);
            generated_c_hits.push(generated_c_hit);
            modules.push((node, emitted));
        }
        let (bootstrap, bootstrap_program) = native_builtin_bootstrap_module(
            &emission,
            options.node,
            group_mask & kiln_backend_c::facts::method_groups::URL != 0,
        )?;
        if !symbols.insert(bootstrap.symbol.clone()) {
            return Err(NativeError::SymbolCollision {
                symbol: bootstrap.symbol,
            });
        }
        let launcher = graph_launcher_source_with_imports(
            graph,
            &modules,
            &options.realm,
            Some(&demand.live_imports),
            Some(&selected.inactive_commonjs_dependencies),
            Some(&selected.partial_namespace_members),
            Some(&native_plan.failures),
            Some(&native_plan.runtime_failures),
            Some(&bootstrap),
        )?;
        let emit_elapsed = emit_started.elapsed();
        let runtime = runtime_source();

        let workspace = WorkDirectory::create()?;
        let mut symbol_list = Vec::new();
        let mut module_sources = Vec::new();
        let mut module_generic_only = Vec::new();
        let mut module_chunk_ranges = Vec::with_capacity(modules.len());
        for ((_, module), &hit) in modules.iter().zip(&generated_c_hits) {
            let start = symbol_list.len();
            if module.chunks.is_empty() {
                symbol_list.push(module.symbol.clone());
                module_sources.push((
                    module.source.as_bytes().to_vec(),
                    Digest::hash(module.source.as_bytes()),
                    hit,
                ));
                module_generic_only.push(module.generic_only);
            } else {
                for (chunk_idx, chunk_source) in module.chunks.iter().enumerate() {
                    symbol_list.push(format!("{}_chunk_{chunk_idx}", module.symbol));
                    module_sources.push((
                        chunk_source.as_bytes().to_vec(),
                        Digest::hash(chunk_source.as_bytes()),
                        hit,
                    ));
                    module_generic_only.push(module.generic_only);
                }
            }
            module_chunk_ranges.push(start..symbol_list.len());
        }
        symbol_list.push(bootstrap.symbol.clone());
        let paths = GraphBuildPaths::new(workspace.path(), &symbol_list);
        let profile = options.profile;
        let compiler = compiler_identity(&identity);
        let bootstrap_capabilities =
            kiln_liveness::program_provider_capabilities(&bootstrap_program, None);
        let required_provider_capabilities =
            kiln_liveness::retained_provider_capabilities(graph, &demand, &members)
                .union(bootstrap_capabilities);
        let (number_provider, number_provider_digest, _, provider_capabilities) =
            self.cached_number_provider_for(&target, required_provider_capabilities)?;

        let bootstrap_source_key = NativeArtifactFamily::ModuleSource.source_key(
            &target,
            Digest::hash(b"native-builtin-bootstrap-c11"),
            [Digest::hash(bootstrap.source.as_bytes())],
        );
        module_sources
            .push(self.cached_payload(&bootstrap_source_key, bootstrap.source.as_bytes())?);
        module_generic_only.push(bootstrap.generic_only);
        let runtime_source_key = NativeArtifactFamily::RuntimeSource.source_key(
            &target,
            Digest::hash(b"portable-runtime-c11"),
            [Digest::hash(runtime.as_bytes())],
        );
        let (runtime_bytes, runtime_source_digest, runtime_generated_c_cache_hit) =
            self.cached_payload(&runtime_source_key, runtime.as_bytes())?;
        let prelude_source = kiln_backend_c::runtime_prelude_source_with_policy(&emission);
        let prelude_source_key = NativeArtifactFamily::PreludeSource.source_key(
            &target,
            Digest::hash(b"portable-prelude-c11"),
            [Digest::hash(prelude_source.as_bytes())],
        );
        let (prelude_bytes, prelude_source_digest, _) =
            self.cached_payload(&prelude_source_key, prelude_source.as_bytes())?;

        let launcher_source_key = NativeArtifactFamily::GraphLauncherSource.source_key(
            &target,
            Digest::hash(b"portable-graph-launcher-c11"),
            [
                Digest::hash(launcher.as_bytes()),
                graph_topology_digest(
                    graph,
                    &modules,
                    Some(&native_plan.failures),
                    Some(&native_plan.runtime_failures),
                ),
            ],
        );
        let (launcher_bytes, launcher_source_digest, launcher_generated_c_cache_hit) =
            self.cached_payload(&launcher_source_key, launcher.as_bytes())?;

        let march_opt = if options.march_native {
            "\0march-native"
        } else {
            ""
        };
        let pgo_opt = pgo_options_fingerprint(options);
        let object_options = Digest::hash(
            format!(
                "c11\0{}\0{}\0generator-{C_GENERATOR_VERSION}{march_opt}{pgo_opt}",
                identity.kind,
                profile.as_str()
            )
            .as_bytes(),
        );
        let compile_options = CompileOptions {
            profile: profile.toolchain(),
            march_native: options.march_native,
            pgo_generate: options.pgo_generate.clone(),
            pgo_use: options.pgo_use.clone(),
            ..CompileOptions::default()
        };
        let mut prelude_compile_options = compile_options.clone();
        if matches!(
            profile,
            NativeProfile::Release | NativeProfile::ReleaseSize | NativeProfile::Profile
        ) {
            // Release CommonJS dispatchers are specialized to this same observable value below.
            // Keep the runtime fact and graph selection one profile decision, or an omitted
            // development arm could become reachable without its module being linked.
            prelude_compile_options
                .extra_args
                .push("-DKILN_NODE_ENV_PRODUCTION=1".into());
        }
        if matches!(profile, NativeProfile::Release | NativeProfile::ReleaseSize) {
            configure_release_prelude_compile(&mut prelude_compile_options);
        }
        // Dispatch groups the program cannot reach compile out of the prelude
        // (see the `#if KILN_RETAIN_GROUP_*` guards). A full mask yields no
        // flags: fully retained builds compile exactly today's prelude.
        prelude_compile_options
            .extra_args
            .extend(prelude_group_defines(group_mask));
        let module_compile_options = module_generic_only
            .iter()
            .map(|&generic_only| {
                let mut compile = compile_options.clone();
                if profile == NativeProfile::Release {
                    configure_release_module_compile(&mut compile, generic_only);
                } else if profile == NativeProfile::ReleaseSize {
                    configure_release_size_module_compile(&mut compile);
                }
                compile
            })
            .collect::<Vec<_>>();
        let module_object_options = module_generic_only
            .iter()
            .map(|&generic_only| {
                Digest::hash(
                    format!(
                        "c11\0{}\0{}\0generator-{C_GENERATOR_VERSION}{march_opt}\0module-opt={}",
                        identity.kind,
                        profile.as_str(),
                        if profile == NativeProfile::Release {
                            release_module_compile_identity(generic_only)
                        } else if profile == NativeProfile::ReleaseSize {
                            release_size_module_compile_identity().to_string()
                        } else {
                            "profile-default".to_string()
                        }
                    )
                    .as_bytes(),
                )
            })
            .collect::<Vec<_>>();
        let compile_started = Instant::now();
        let mut module_keys = Vec::with_capacity(module_sources.len());
        for index in 0..module_sources.len() {
            let (_, source_digest, _) = &module_sources[index];
            module_keys.push(NativeArtifactFamily::ModuleObject.key(
                &compiler,
                &target,
                profile.cache(),
                module_object_options[index],
                [*source_digest, prelude_header_digest()],
            ));
        }
        let prelude_key = NativeArtifactFamily::RuntimePreludeLibrary.key(
            &compiler,
            &target,
            profile.cache(),
            prelude_object_options_masked(object_options, profile, group_mask),
            [prelude_source_digest],
        );
        let runtime_key = NativeArtifactFamily::RuntimeLibrary.key(
            &compiler,
            &target,
            profile.cache(),
            object_options,
            [runtime_source_digest],
        );
        let launcher_key = NativeArtifactFamily::GraphLauncherObject.key(
            &compiler,
            &target,
            profile.cache(),
            object_options,
            [launcher_source_digest],
        );

        let link_options_digest = executable_link_options_digest(profile, options);

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
            let candidate_executable_key = NativeArtifactFamily::ExecutableLink.key(
                &compiler,
                &target,
                profile.cache(),
                link_options_digest,
                link_inputs,
            );
            if let Lookup::Hit(artifact) = self.lookup(&candidate_executable_key)? {
                debug_assert_eq!(artifact.digest, Digest::hash(&artifact.bytes));
                materialize(&options.output, &artifact.bytes, true)?;
                let mut module_artifacts = Vec::with_capacity(modules.len());
                for (index, range) in module_chunk_ranges.iter().enumerate() {
                    let gen_c_bytes: u64 = module_sources[range.clone()]
                        .iter()
                        .map(|(s, _, _)| s.len() as u64)
                        .sum();
                    let obj_bytes: u64 = module_object_infos[range.clone()]
                        .iter()
                        .map(|(_, size)| *size)
                        .sum();
                    let gen_hit = module_sources[range.clone()].iter().all(|(_, _, hit)| *hit);
                    module_artifacts.push(ModuleArtifactInfo {
                        logical_path: Some(logical_paths[index].clone()),
                        generated_c_cache_hit: gen_hit,
                        object_cache_hit: true,
                        generated_c_bytes: gen_c_bytes,
                        object_bytes: obj_bytes,
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
                        prelude_object: true,
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
                        prelude_object: prelude_info.1,
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
                    provider_capabilities,
                    required_provider_capabilities,
                });
            }
        }

        materialize_prelude_header(workspace.path())?;
        let (module_objects, ((prelude_object, runtime_object), launcher_object)) = rayon::join(
            || {
                let module_keys_ref = &module_keys;
                let module_sources_ref = &module_sources;
                let module_c_ref = &paths.module_c;
                let module_o_ref = &paths.module_o;
                let compile_options_ref = &module_compile_options;
                let sizes: Vec<_> = module_sources_ref
                    .iter()
                    .map(|(source, _, _)| source.len())
                    .collect();
                compile_largest_first(&sizes, "module compile", move |index| {
                    self.object(
                        &module_keys_ref[index],
                        &module_sources_ref[index].0,
                        &module_c_ref[index],
                        &module_o_ref[index],
                        &compile_options_ref[index],
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
                                        &prelude_compile_options,
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

        let link_options_digest = executable_link_options_digest(profile, options);
        let mut link_inputs = module_objects
            .iter()
            .map(|(_, digest, _)| *digest)
            .collect::<Vec<_>>();
        link_inputs.push(prelude_digest);
        link_inputs.push(runtime_digest);
        link_inputs.push(launcher_digest);
        link_inputs.push(number_provider_digest);
        let executable_key = NativeArtifactFamily::ExecutableLink.key(
            &compiler,
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
                let mut objects = Vec::with_capacity(module_objects.len() + 3);
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
                    static_link: options.static_link,
                    pgo_generate: options.pgo_generate.clone(),
                    pgo_use: options.pgo_use.clone(),
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

        let module_artifacts = module_chunk_ranges
            .iter()
            .enumerate()
            .map(|(index, range)| {
                let gen_c_bytes: u64 = module_sources[range.clone()]
                    .iter()
                    .map(|(s, _, _)| s.len() as u64)
                    .sum();
                let obj_bytes: u64 = module_objects[range.clone()]
                    .iter()
                    .map(|(o, _, _)| o.len() as u64)
                    .sum();
                let gen_hit = module_sources[range.clone()].iter().all(|(_, _, hit)| *hit);
                let obj_hit = module_objects[range.clone()].iter().all(|(_, _, hit)| *hit);
                ModuleArtifactInfo {
                    logical_path: Some(logical_paths[index].clone()),
                    generated_c_cache_hit: gen_hit,
                    object_cache_hit: obj_hit,
                    generated_c_bytes: gen_c_bytes,
                    object_bytes: obj_bytes,
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
                prelude_object: prelude_hit,
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
                prelude_object: prelude_object.len() as u64,
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
            provider_capabilities,
            required_provider_capabilities,
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
        if identities.contains(&key.identity_digest()) {
            return;
        }
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
                if std::env::var_os("KILN_NATIVE_TRACE_COMPILE").is_some() {
                    eprintln!(
                        "kiln: C compile {}ms {}",
                        report.elapsed.as_millis(),
                        source_path
                            .file_name()
                            .unwrap_or(source_path.as_os_str())
                            .to_string_lossy()
                    );
                }
                let bytes = read(object_path, "read native object")?;
                let stored = self.cache.put(key, &bytes)?;
                Ok((bytes, stored.digest, false, report.elapsed))
            }
        }
    }
}

#[derive(Debug, Default)]
struct NativeGraphPlan {
    excluded: BTreeSet<PathBuf>,
    failures: BTreeMap<(PathBuf, String), String>,
    runtime_failures: BTreeMap<(PathBuf, PathBuf, String), String>,
}

fn native_graph_plan(graph: &ModuleGraph) -> Result<NativeGraphPlan, NativeError> {
    native_graph_plan_with_validator(graph, &validate_native_builtin_node)
}

fn native_graph_plan_with_validator(
    graph: &ModuleGraph,
    validate: &dyn Fn(&ModuleGraph, &kiln_module_graph::ModuleNode) -> Result<(), NativeError>,
) -> Result<NativeGraphPlan, NativeError> {
    #[derive(Clone)]
    struct LazyEdge {
        owner: PathBuf,
        specifier: String,
        captured_importer: PathBuf,
        target: PathBuf,
        runtime: bool,
    }

    let static_reachable = graph.statically_reachable();
    validate_native_builtins_in_paths(graph, &static_reachable, validate)?;
    let mut plan = NativeGraphPlan::default();
    let mut retained = static_reachable.clone();
    let mut pending = static_reachable
        .iter()
        .flat_map(|path| {
            let dynamic = graph
                .dynamic_dependencies(path)
                .iter()
                .map(move |dependency| LazyEdge {
                    owner: path.clone(),
                    specifier: dependency.specifier.clone(),
                    captured_importer: path.clone(),
                    target: dependency.target.clone(),
                    runtime: false,
                });
            let runtime = graph.module(path).into_iter().flat_map(move |node| {
                node.runtime_dependencies
                    .iter()
                    .map(move |dependency| LazyEdge {
                        owner: path.clone(),
                        specifier: dependency.specifier.clone(),
                        captured_importer: dependency.importer.clone(),
                        target: dependency.target.clone(),
                        runtime: true,
                    })
            });
            dynamic.chain(runtime)
        })
        .collect::<Vec<_>>();
    let mut processed = BTreeSet::new();
    while let Some(edge) = pending.pop() {
        if !processed.insert((
            edge.owner.clone(),
            edge.captured_importer.clone(),
            edge.specifier.clone(),
            edge.runtime,
        )) {
            continue;
        }
        let mut closure = BTreeSet::new();
        let mut walk = vec![edge.target.clone()];
        let mut unsupported = None;
        while let Some(path) = walk.pop() {
            if !closure.insert(path.clone()) {
                continue;
            }
            let Some(node) = graph.module(&path) else {
                continue;
            };
            if let Err(error) = validate(graph, node) {
                unsupported.get_or_insert(error.to_string());
            }
            walk.extend(node.dependencies.iter().map(|edge| edge.target.clone()));
        }
        if let Some(message) = unsupported {
            if edge.runtime {
                plan.runtime_failures.insert(
                    (edge.owner, edge.captured_importer, edge.specifier),
                    format!("runtime request failed: {message}"),
                );
            } else {
                plan.failures.insert(
                    (edge.owner, edge.specifier),
                    format!("dynamic import failed: {message}"),
                );
            }
            continue;
        }
        for path in &closure {
            retained.insert(path.clone());
            for nested in graph.dynamic_dependencies(path) {
                pending.push(LazyEdge {
                    owner: path.clone(),
                    specifier: nested.specifier.clone(),
                    captured_importer: path.clone(),
                    target: nested.target.clone(),
                    runtime: false,
                });
            }
            if let Some(node) = graph.module(path) {
                for nested in &node.runtime_dependencies {
                    pending.push(LazyEdge {
                        owner: path.clone(),
                        specifier: nested.specifier.clone(),
                        captured_importer: nested.importer.clone(),
                        target: nested.target.clone(),
                        runtime: true,
                    });
                }
            }
        }
    }
    plan.excluded = graph
        .modules()
        .map(|node| node.path.clone())
        .filter(|path| !retained.contains(path))
        .collect();
    Ok(plan)
}

fn validate_native_builtins_in_paths(
    graph: &ModuleGraph,
    paths: &BTreeSet<PathBuf>,
    validate: &dyn Fn(&ModuleGraph, &kiln_module_graph::ModuleNode) -> Result<(), NativeError>,
) -> Result<(), NativeError> {
    for path in paths {
        if let Some(node) = graph.module(path) {
            validate(graph, node)?;
        }
    }
    Ok(())
}

fn validate_native_builtin_node(
    graph: &ModuleGraph,
    node: &kiln_module_graph::ModuleNode,
) -> Result<(), NativeError> {
    for import in &node.program.imports {
        let Some(dependency) = node
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
    Ok(())
}

fn native_builtin_supports_export(
    builtin: kiln_module_resolver::NodeBuiltin,
    export_name: &str,
) -> bool {
    kiln_builtins::builtin_supported_exports(builtin).contains(&export_name)
}

const NATIVE_BUILTIN_BOOTSTRAP_SOURCE: &str = r#"
const __kilnSegmentBoundaries = globalThis.__kilnIntlSegment;
delete globalThis.__kilnIntlSegment;

function Segmenter(locales, options) {
    const granularity = options === undefined || options.granularity === undefined
        ? "grapheme"
        : String(options.granularity);
    if (granularity !== "grapheme" && granularity !== "word" && granularity !== "sentence") {
        throw new RangeError("invalid granularity: " + granularity);
    }
    this.__kilnGranularity = granularity;
    this.__kilnLocale = typeof locales === "string" ? locales : "en-US";
}

Segmenter.prototype.segment = function (input) {
    const text = String(input);
    const kind = this.__kilnGranularity === "word"
        ? 1
        : (this.__kilnGranularity === "sentence" ? 2 : 0);
    const tape = __kilnSegmentBoundaries(text, kind);
    const records = [];
    for (let index = 0; index < tape.length; index += 4) {
        const record = {
            segment: text.slice(tape[index], tape[index + 1]),
            index: tape[index],
            input: text
        };
        if (tape[index + 2] !== 0) record.isWordLike = tape[index + 3] !== 0;
        records.push(record);
    }
    const segments = {
        containing(index) {
            const position = Number(index);
            if (!Number.isFinite(position) || position < 0 || position >= text.length) {
                return undefined;
            }
            for (let at = 0; at < records.length; at += 1) {
                const start = records[at].index;
                const end = start + records[at].segment.length;
                if (start <= position && position < end) return records[at];
            }
            return undefined;
        }
    };
    segments[Symbol.iterator] = function () { return records[Symbol.iterator](); };
    return segments;
};

Segmenter.prototype.resolvedOptions = function () {
    return { locale: this.__kilnLocale, granularity: this.__kilnGranularity };
};

Segmenter.supportedLocalesOf = function (locales) {
    if (locales === undefined) return [];
    return Array.isArray(locales) ? locales.slice() : [String(locales)];
};

Intl.Segmenter = Segmenter;


const __kilnUint8ToHex = globalThis.__kilnUint8ToHex;
delete globalThis.__kilnUint8ToHex;
const __kilnUint8ToBase64 = globalThis.__kilnUint8ToBase64;
delete globalThis.__kilnUint8ToBase64;

if (typeof Uint8Array !== "undefined" && Uint8Array.prototype) {
    Uint8Array.prototype.toHex = function toHex() {
        if (!(this instanceof Uint8Array)) {
            throw new TypeError("toHex called on incompatible receiver");
        }
        return __kilnUint8ToHex(this);
    };

    Uint8Array.prototype.toBase64 = function toBase64(options) {
        if (!(this instanceof Uint8Array)) {
            throw new TypeError("toBase64 called on incompatible receiver");
        }
        let url = false;
        let omitPadding = false;
        if (options !== undefined) {
            if (typeof options !== "object" || options === null) {
                throw new TypeError("options must be an object");
            }
            if (options.alphabet !== undefined) {
                const alphabet = String(options.alphabet);
                if (alphabet === "base64url") {
                    url = true;
                } else if (alphabet === "base64") {
                    url = false;
                } else {
                    throw new TypeError('alphabet must be "base64" or "base64url"');
                }
            }
            omitPadding = Boolean(options.omitPadding);
        }
        return __kilnUint8ToBase64(this, url, omitPadding);
    };
}

if (typeof JSON !== "undefined") {
    const __kilnOriginalParse = JSON.parse;
    const __kilnOriginalStringify = JSON.stringify;

    JSON.parse = function parse(text, reviver) {
        const root = __kilnOriginalParse(text);
        if (typeof reviver !== "function") return root;

        function internalize(holder, name) {
            const val = holder[name];
            if (val && typeof val === "object") {
                if (Array.isArray(val)) {
                    for (let i = 0; i < val.length; i++) {
                        const newElement = internalize(val, String(i));
                        if (newElement === undefined) {
                            delete val[i];
                        } else {
                            val[i] = newElement;
                        }
                    }
                } else {
                    for (const key of Object.keys(val)) {
                        const newProperty = internalize(val, key);
                        if (newProperty === undefined) {
                            delete val[key];
                        } else {
                            val[key] = newProperty;
                        }
                    }
                }
            }
            return reviver.call(holder, name, val);
        }

        return internalize({ "": root }, "");
    };

    JSON.stringify = function stringify(value, replacer, space) {
        if (typeof replacer !== "function" && !Array.isArray(replacer)) {
            return __kilnOriginalStringify(value, replacer, space);
        }
        let replacerFn = null;
        let propertyList = null;
        if (typeof replacer === "function") {
            replacerFn = replacer;
        } else if (Array.isArray(replacer)) {
            propertyList = [];
            for (let i = 0; i < replacer.length; i++) {
                const item = replacer[i];
                if (typeof item === "string" || typeof item === "number") {
                    const str = String(item);
                    if (propertyList.indexOf(str) === -1) {
                        propertyList.push(str);
                    }
                }
            }
        }

        const stack = [];
        function serialize(holder, key) {
            let val = holder[key];
            if (val && typeof val === "object" && typeof val.toJSON === "function") {
                val = val.toJSON(key);
            }
            if (replacerFn !== null) {
                val = replacerFn.call(holder, key, val);
            }
            if (val === undefined || typeof val === "function" || typeof val === "symbol") {
                return undefined;
            }
            if (val === null || typeof val !== "object") {
                return val;
            }
            if (stack.indexOf(val) !== -1) {
                throw new TypeError("Converting circular structure to JSON");
            }
            stack.push(val);
            let res;
            if (Array.isArray(val)) {
                res = [];
                for (let i = 0; i < val.length; i++) {
                    const item = serialize(val, String(i));
                    res.push(item === undefined ? null : item);
                }
            } else {
                res = {};
                const keys = propertyList !== null ? propertyList : Object.keys(val);
                for (let i = 0; i < keys.length; i++) {
                    const k = keys[i];
                    if (Object.prototype.hasOwnProperty.call(val, k)) {
                        const propVal = serialize(val, k);
                        if (propVal !== undefined) {
                            res[k] = propVal;
                        }
                    }
                }
            }
            stack.pop();
            return res;
        }

        const transformed = serialize({ "": value }, "");
        if (transformed === undefined) return undefined;
        return __kilnOriginalStringify(transformed, undefined, space);
    };
}

// The rest of `console`. The prelude C builds the global with `log`, `info`, `error` and `warn`
// and nothing else, while the interpreter's carries the twenty-four the language does plus a
// `Console` prototype. Every other method read `undefined` natively -- and `console.debug("x")`
// then died three frames later as "undefined is not a function" -- until 2026-09-19. What each
// does is the interpreter's rule, copied: `debug`, `dir`, `table` print as `log` does; `assert`
// and `trace` print to stderr under a fixed prefix; the timing, grouping, counting and profiling
// methods accept their arguments and do nothing, because the realm's clock is virtual
// (ADR 0044) and there is no debugger to speak to. Written here rather than in C per the
// Phase 5.1 freeze.
{
    const c = globalThis.console;
    // An instance made with `new Console(stdout[, stderr])` writes to those streams, as the
    // interpreter's does; the global `console` carries no streams and reaches the prelude's
    // `log` and `error`, which is where every method here ends up for it.
    const out = (self, stderr, args) => {
        const stream = stderr ? self._stderr : self._stdout;
        if (stream && typeof stream.write === "function") {
            stream.write(args.map(String).join(" ") + "\n");
        } else if (stderr) {
            c.error(...args);
        } else {
            c.log(...args);
        }
    };
    class Console {
        constructor(stdout, stderr) {
            let o = stdout;
            let e = stderr;
            if (o && typeof o === "object" && o.stdout) {
                e = o.stderr || o.stdout;
                o = o.stdout;
            }
            if (o === undefined) {
                throw new TypeError("Console expects a writable stream instance for stdout");
            }
            this._stdout = o;
            this._stderr = e || o;
        }
        log(...args) { out(this, false, args); }
        info(...args) { out(this, false, args); }
        debug(...args) { out(this, false, args); }
        dir(...args) { out(this, false, args); }
        table(...args) { out(this, false, args); }
        error(...args) { out(this, true, args); }
        warn(...args) { out(this, true, args); }
        // The interpreter formats the message first (`util.format`, so `%s` in its first
        // argument substitutes) and prefixes the result. Folding the prefix into a string first
        // argument is the same thing; a non-string first argument is inspected either way.
        assert(value, ...message) {
            if (value) return;
            if (message.length === 0) out(this, true, ["Assertion failed"]);
            else if (typeof message[0] === "string") {
                out(this, true, ["Assertion failed: " + message[0], ...message.slice(1)]);
            } else out(this, true, ["Assertion failed:", ...message]);
        }
        trace(...args) {
            if (args.length === 0) out(this, true, ["Trace"]);
            else if (typeof args[0] === "string") out(this, true, ["Trace: " + args[0], ...args.slice(1)]);
            else out(this, true, ["Trace:", ...args]);
        }
        clear() {}
        count() {}
        countReset() {}
        group() {}
        groupEnd() {}
        groupCollapsed() {}
        time() {}
        timeEnd() {}
        timeLog() {}
        dirxml() {}
        timeStamp() {}
        profile() {}
        profileEnd() {}
    }
    const proto = Console.prototype;
    Object.defineProperty(proto, Symbol.toStringTag, { value: "console", configurable: true });
    proto.Console = Console;
    const added = ["debug", "dir", "table", "assert", "trace", "clear", "count", "countReset",
                   "group", "groupEnd", "groupCollapsed", "time", "timeEnd", "timeLog", "dirxml",
                   "timeStamp", "profile", "profileEnd"];
    for (let i = 0; i < added.length; i++) {
        Object.defineProperty(c, added[i], { value: proto[added[i]], writable: true,
                                             configurable: true, enumerable: false });
    }
    Object.defineProperty(c, "Console", { value: Console, writable: true, configurable: true,
                                          enumerable: false });
    Object.setPrototypeOf(c, proto);
}

// Members the prelude C answers `undefined` for off a built-in's prototype, defined into the
// built-in's own store, which every read consults first. `Error.prototype.name` is the one that
// matters most: `class X extends Error {}` gives an instance whose chain ends here, so
// `new X().name` was `undefined` where node says "Error". The `@@`-keyed methods existed in the
// interpreter and read `undefined` natively for as long as the symbol catch-all stood; with that
// gone they would refuse, and these are the answers. Each is written the way the specification
// reduces it, to the ordinary method on the same receiver.
{
    const data = (target, key, value, writable) =>
        Object.defineProperty(target, key, { value, writable: writable === true,
                                             configurable: true, enumerable: false });
    data(Error.prototype, "name", "Error", true);
    data(Error.prototype, "message", "", true);
    data(AggregateError.prototype, "name", "AggregateError", true);
    data(AggregateError.prototype, "message", "", true);
    Object.defineProperty(String.prototype, "length", { value: 0, writable: false,
                                                        configurable: false, enumerable: false });
    data(ArrayBuffer.prototype, Symbol.toStringTag, "ArrayBuffer");
    data(DataView.prototype, Symbol.toStringTag, "DataView");
    const methods = {
        [Symbol.match](string) { return String(string).match(this); },
        [Symbol.matchAll](string) { return String(string).matchAll(this); },
        [Symbol.replace](string, replacement) { return String(string).replace(this, replacement); },
        [Symbol.search](string) { return String(string).search(this); },
        [Symbol.split](string, limit) { return String(string).split(this, limit); },
    };
    data(RegExp.prototype, Symbol.match, methods[Symbol.match], true);
    data(RegExp.prototype, Symbol.matchAll, methods[Symbol.matchAll], true);
    data(RegExp.prototype, Symbol.replace, methods[Symbol.replace], true);
    data(RegExp.prototype, Symbol.search, methods[Symbol.search], true);
    data(RegExp.prototype, Symbol.split, methods[Symbol.split], true);
    const symbolToPrimitive = {
        [Symbol.toPrimitive]() { return this.valueOf(); },
    };
    data(Symbol.prototype, Symbol.toPrimitive, symbolToPrimitive[Symbol.toPrimitive]);
    const hasInstance = {
        [Symbol.hasInstance](value) {
            if (typeof this !== "function") return false;
            if (value === null || (typeof value !== "object" && typeof value !== "function")) {
                return false;
            }
            const target = this.prototype;
            let current = Object.getPrototypeOf(value);
            while (current !== null) {
                if (current === target) return true;
                current = Object.getPrototypeOf(current);
            }
            return false;
        },
    };
    data(Function.prototype, Symbol.hasInstance, hasInstance[Symbol.hasInstance]);
}

// The ES2025 Set methods, over the Set intrinsics. `other` is a set-like: anything with a
// numeric `size`, a callable `has` and a callable `keys` (GetSetRecord), which is what lets a
// Map stand in for a Set of its keys. Each method reads `this` through `has`/`keys` and never
// through the other side's `has` when the specification says to iterate instead, because the
// argument's `has` can lie and the result must be what the specification says, not what the
// argument claims.
{
    const setRecord = (other) => {
        if (other === null || (typeof other !== "object" && typeof other !== "function")) {
            throw new TypeError("The \"other\" argument must be set-like");
        }
        const rawSize = other.size;
        const size = Number(rawSize);
        if (rawSize === undefined || Number.isNaN(size)) {
            throw new TypeError("The \"other\" argument must have a numeric \"size\" property");
        }
        if (typeof other.has !== "function") {
            throw new TypeError("The \"other\" argument must have a callable \"has\" method");
        }
        if (typeof other.keys !== "function") {
            throw new TypeError("The \"other\" argument must have a callable \"keys\" method");
        }
        return { set: other, size: Math.trunc(size), has: other.has, keys: other.keys };
    };
    const requireSet = (value, name) => {
        if (!(value instanceof Set)) {
            throw new TypeError(`Method Set.prototype.${name} called on incompatible receiver`);
        }
    };
    const methods = {
        union(other) {
            requireSet(this, "union");
            const record = setRecord(other);
            const result = new Set(this);
            for (const value of record.keys.call(record.set)) result.add(value);
            return result;
        },
        intersection(other) {
            requireSet(this, "intersection");
            const record = setRecord(other);
            const result = new Set();
            if (this.size <= record.size) {
                for (const value of this) if (record.has.call(record.set, value)) result.add(value);
            } else {
                for (const value of record.keys.call(record.set)) if (this.has(value)) result.add(value);
            }
            return result;
        },
        difference(other) {
            requireSet(this, "difference");
            const record = setRecord(other);
            const result = new Set(this);
            if (this.size <= record.size) {
                for (const value of this) if (record.has.call(record.set, value)) result.delete(value);
            } else {
                for (const value of record.keys.call(record.set)) result.delete(value);
            }
            return result;
        },
        symmetricDifference(other) {
            requireSet(this, "symmetricDifference");
            const record = setRecord(other);
            const result = new Set(this);
            for (const value of record.keys.call(record.set)) {
                if (this.has(value)) result.delete(value);
                else result.add(value);
            }
            return result;
        },
        isSubsetOf(other) {
            requireSet(this, "isSubsetOf");
            const record = setRecord(other);
            if (this.size > record.size) return false;
            for (const value of this) if (!record.has.call(record.set, value)) return false;
            return true;
        },
        isSupersetOf(other) {
            requireSet(this, "isSupersetOf");
            const record = setRecord(other);
            if (this.size < record.size) return false;
            for (const value of record.keys.call(record.set)) if (!this.has(value)) return false;
            return true;
        },
        isDisjointFrom(other) {
            requireSet(this, "isDisjointFrom");
            const record = setRecord(other);
            if (this.size <= record.size) {
                for (const value of this) if (record.has.call(record.set, value)) return false;
            } else {
                for (const value of record.keys.call(record.set)) if (this.has(value)) return false;
            }
            return true;
        },
    };
    for (const name of Object.keys(methods)) {
        Object.defineProperty(Set.prototype, name, { value: methods[name], writable: true,
                                                     configurable: true, enumerable: false });
    }
}

// `structuredClone`, the HTML structured clone over the values this runtime has: primitives,
// Date, RegExp, Boolean/Number/String objects, Error, ArrayBuffer and the typed arrays, Map,
// Set, arrays and plain objects, with cycles preserved. A function, a symbol, a WeakMap or a
// Proxy is a DataCloneError, as in node. The interpreter has its own in Rust; the emitter reads
// this one by name (`BuiltinLoad::Global`).
{
    const isTypedArray = (v) => ArrayBuffer.isView(v) && !(v instanceof DataView);
    function DataCloneError(message) {
        const err = new Error(message);
        err.name = "DataCloneError";
        err.code = 25;
        return err;
    }
    function clone(value, seen) {
        if (value === null || (typeof value !== "object" && typeof value !== "function")) {
            if (typeof value === "symbol") throw DataCloneError(String(value) + " could not be cloned.");
            return value;
        }
        if (typeof value === "function") throw DataCloneError(String(value).split("\n")[0] + " could not be cloned.");
        if (seen.has(value)) return seen.get(value);
        let out;
        if (value instanceof Date) { out = new Date(value.getTime()); seen.set(value, out); return out; }
        if (value instanceof RegExp) { out = new RegExp(value.source, value.flags); seen.set(value, out); return out; }
        if (value instanceof Boolean || value instanceof Number || value instanceof String) {
            out = Object(value.valueOf()); seen.set(value, out); return out;
        }
        if (value instanceof Error) {
            const ctor = { EvalError, RangeError, ReferenceError, SyntaxError, TypeError, URIError }[value.name] || Error;
            out = new ctor(value.message);
            seen.set(value, out);
            if ("stack" in value) out.stack = value.stack;
            if ("cause" in value) out.cause = clone(value.cause, seen);
            return out;
        }
        if (value instanceof ArrayBuffer) { out = value.slice(0); seen.set(value, out); return out; }
        if (isTypedArray(value)) {
            out = new value.constructor(value); seen.set(value, out); return out;
        }
        if (value instanceof DataView) {
            out = new DataView(value.buffer.slice(0), value.byteOffset, value.byteLength); seen.set(value, out); return out;
        }
        if (value instanceof Map) {
            out = new Map(); seen.set(value, out);
            for (const [k, v] of value) out.set(clone(k, seen), clone(v, seen));
            return out;
        }
        if (value instanceof Set) {
            out = new Set(); seen.set(value, out);
            for (const v of value) out.add(clone(v, seen));
            return out;
        }
        if (Array.isArray(value)) {
            out = new Array(value.length); seen.set(value, out);
            for (let i = 0; i < value.length; i++) if (i in value) out[i] = clone(value[i], seen);
            return out;
        }
        if (value instanceof WeakMap || value instanceof WeakSet || value instanceof Promise) {
            throw DataCloneError('#<' + value.constructor.name + "> could not be cloned.");
        }
        out = {}; seen.set(value, out);
        for (const key of Object.keys(value)) out[key] = clone(value[key], seen);
        return out;
    }
    function structuredClone(value) {
        if (arguments.length === 0) throw new TypeError("The \"value\" argument must be specified");
        return clone(value, new Map());
    }
    Object.defineProperty(globalThis, "structuredClone", { value: structuredClone, writable: true, configurable: true, enumerable: true });
}

// `Object.groupBy` and `Map.groupBy` (ES2024), over the iteration protocol; both refused by
// name natively until 2026-09-19. Defined into the constructors' stores, which every read of a
// built-in consults first.
{
    const data = (target, key, value) =>
        Object.defineProperty(target, key, { value, writable: true, configurable: true, enumerable: false });
    data(Object, "groupBy", function groupBy(items, callback) {
        if (typeof callback !== "function") throw new TypeError(String(callback) + " is not a function");
        const out = Object.create(null);
        let k = 0;
        for (const item of items) {
            const key = callback(item, k++);
            const name = typeof key === "symbol" ? key : String(key);
            if (!(name in out)) out[name] = [];
            out[name].push(item);
        }
        return out;
    });
    data(Map, "groupBy", function groupBy(items, callback) {
        if (typeof callback !== "function") throw new TypeError(String(callback) + " is not a function");
        const out = new Map();
        let k = 0;
        for (const item of items) {
            let key = callback(item, k++);
            if (key === 0) key = 0;
            const list = out.get(key);
            if (list === undefined) out.set(key, [item]);
            else list.push(item);
        }
        return out;
    });
}
"#;

#[cfg(test)]
fn emit_native_graph_module(
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &NativeRealmPolicy,
) -> Result<kiln_backend_c::CModule, NativeError> {
    emit_native_graph_module_with_policy(node, logical_path, &policy.emission())
}

// This wrapper decides which backend entry to call. Its source joins the backend source
// fingerprint so an uncommitted emitter edit cannot be hidden by an input-cache hit.
static GRAPH_EMISSION_WRAPPER_ID: std::sync::LazyLock<Digest> =
    std::sync::LazyLock::new(|| Digest::hash(include_str!("lib.rs").as_bytes()));

/// Cache before graph construction/C rendering, rather than hashing the rendered C first.
/// Only ordinary code modules with the split prelude use this cache: builtin materializers,
/// JSON and self-contained runtime bodies continue through their existing source identities.
fn cached_graph_module(
    cache: &ArtifactCache,
    target: &str,
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &EmissionPolicy,
    emit: impl FnOnce() -> Result<kiln_backend_c::CModule, NativeError>,
) -> Result<(kiln_backend_c::CModule, bool), NativeError> {
    if !policy.decoupled_prelude
        || !matches!(node.source, ModuleSource::Source | ModuleSource::CommonJs)
    {
        return emit().map(|module| (module, false));
    }
    // These owned IR/policy types use derived Debug over ordered fields, Vecs and BTreeSets;
    // there are no addresses or unordered maps. Hash the complete representation so additions
    // to IR or policy automatically participate, including source spans, strictness, exports,
    // type profiles and closed-world liveness. This is a versioned internal cache encoding,
    // not a public serialization format. Representation changes can only cause cache misses.
    let input = Digest::hash(
        format!("{:?}", (&node.program, node.source, logical_path, policy)).as_bytes(),
    );
    let cache_key = NativeArtifactFamily::ModuleSource.source_key(
        target,
        Digest::hash(b"native-graph-emission-input-v3"),
        [
            input,
            prelude_header_digest(),
            Digest::hash(kiln_backend_c::C_EMITTER_BUILD_ID.as_bytes()),
            *GRAPH_EMISSION_WRAPPER_ID,
        ],
    );
    match cache.lookup(&cache_key)? {
        Lookup::Hit(artifact) => {
            let (program_hash, symbol, source, generic_only, chunks): (
                u64,
                String,
                String,
                bool,
                Vec<String>,
            ) = serde_json::from_slice(&artifact.bytes).map_err(|error| CacheError::Corrupt {
                path: artifact.path,
                reason: format!("invalid cached C module: {error}"),
            })?;
            Ok((
                kiln_backend_c::CModule {
                    program_hash,
                    symbol,
                    source,
                    generic_only,
                    chunks,
                },
                true,
            ))
        }
        Lookup::Miss => {
            let module = emit()?;
            let bytes = serde_json::to_vec(&(
                module.program_hash,
                &module.symbol,
                &module.source,
                module.generic_only,
                &module.chunks,
            ))
            .expect("C module contains only strings and an integer");
            cache.put(&cache_key, &bytes)?;
            Ok((module, false))
        }
    }
}

fn emit_native_graph_module_with_policy(
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &EmissionPolicy,
) -> Result<kiln_backend_c::CModule, NativeError> {
    match node.source {
        ModuleSource::NodeBuiltin(_builtin) => Ok(emit_module_with_logical_path_and_policy(
            &node.program,
            logical_path,
            policy.clone(),
        )?),
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
            // `kiln_c99_json_document` answers the heap's own `kiln_val`; `exports` is an array
            // of boundary `kiln_value`, which is three times the size. Writing the answer
            // straight into the slot left the tag and length of the export unwritten, and the
            // module then handed out an object tag over a null pointer.
            let units: Vec<u16> = kiln_ir::StringUnits::from(source).units().collect();
            let latin1 = units.iter().all(|unit| *unit <= 0xff);
            let (flags, payload, type_name) = if latin1 {
                let payload = if units.is_empty() {
                    "0".to_owned()
                } else {
                    units
                        .iter()
                        .map(|unit| unit.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                ("KILN_STRING_LATIN1", payload, "uint8_t")
            } else {
                let payload = if units.is_empty() {
                    "0".to_owned()
                } else {
                    units
                        .iter()
                        .map(|unit| format!("UINT16_C({unit})"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                ("KILN_STRING_UTF16", payload, "uint16_t")
            };
            let body = format!(
                "static const {type_name} json_source_data[{}] = {{ {payload} }};\n\
                 static const kiln_string json_source = {{ {}, {flags}, json_source_data }};\n\
                 kiln_val parsed;\n\
                 int status = kiln_c99_json_document(&json_source, &parsed);\n\
                 if (status != KILN_C99_OK) return status;\n\
                 exports[0] = kiln_val_to_boundary(parsed);",
                units.len().max(1),
                units.len()
            );
            Ok(emit_runtime_module_with_logical_path_and_policy(
                &node.program,
                logical_path,
                &body,
                policy.clone(),
            )?)
        }
        ModuleSource::Asset => {
            let path = node.data.as_deref().ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::Source {
                    path: node.path.clone(),
                    message: "asset module has no path metadata".to_owned(),
                })
            })?;
            let body = format!(
                "exports[0] = kiln_c99_copy_text({}, (size_t){});",
                native_c_string(path),
                path.len()
            );
            Ok(emit_runtime_module_with_logical_path_and_policy(
                &node.program,
                logical_path,
                &body,
                policy.clone(),
            )?)
        }
        ModuleSource::Source => Ok(emit_module_with_logical_path_and_policy(
            &node.program,
            logical_path,
            policy.clone(),
        )?),
    }
}

fn native_builtin_bootstrap_module(
    policy: &EmissionPolicy,
    node: bool,
    retain_url: bool,
) -> Result<(kiln_backend_c::CModule, kiln_ir::ModuleProgram), NativeError> {
    const LOGICAL_PATH: &str = "@kiln/runtime-bootstrap.ts";
    // `URL` rides the retain group liveness computed: a program that names neither it nor
    // anything built on it gets no class, and the prelude behind it compiled out. `--node`
    // always has it, because `fetch` is built on it.
    let url = if retain_url || node {
        kiln_builtins::URL_BOOTSTRAP_SOURCE
    } else {
        ""
    };
    let date = kiln_builtins::DATE_BOOTSTRAP_SOURCE;
    let source = if node {
        format!(
            "{NATIVE_BUILTIN_BOOTSTRAP_SOURCE}\n{date}\n{url}\n{}",
            kiln_builtins::WEB_BOOTSTRAP_SOURCE
        )
    } else {
        // The Buffer bootstrap (Node mode) captures and deletes this bridge; without it the
        // bridge would be the one global node does not have, so it goes here instead.
        format!(
            "{NATIVE_BUILTIN_BOOTSTRAP_SOURCE}\n{date}\n{url}\ndelete globalThis.__kilnBufferToString;\n"
        )
    };
    let hir = frontend_oxc::parse(LOGICAL_PATH, &source);
    let program = kiln_ir::compile_module(&hir).map_err(|diagnostics| {
        NativeError::Graph(ModuleGraphError::Compile {
            path: PathBuf::from(LOGICAL_PATH),
            diagnostics,
        })
    })?;
    // The bootstrap is cold code every binary carries -- the console methods, the Set algebra,
    // the URL class -- and emitted like hot user code, with a fast clone and a typed clone per
    // function, it put 0.9 MB on a hello world (2.74 MB against 1.83 MB, measured in a clean
    // cache). The generic path alone is what a program that never calls `console.table` pays for.
    let mut bootstrap_policy = policy.clone();
    bootstrap_policy.generic_only = true;
    let c_module =
        emit_module_with_logical_path_and_policy(&program, LOGICAL_PATH, bootstrap_policy)?;
    Ok((c_module, program))
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

fn digest_graph_path(graph: &ModuleGraph, path: &Path) -> String {
    logical_module_path(graph.project_root(), path)
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

fn append_runtime_digest(
    bytes: &mut Vec<u8>,
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    native_runtime_failures: Option<&BTreeMap<(PathBuf, PathBuf, String), String>>,
) {
    let mut indices = std::collections::HashMap::with_capacity(modules.len());
    for (index, (node, _)) in modules.iter().enumerate() {
        indices.insert(node.path.clone(), index);
    }
    for (node, _) in modules {
        for dependency in &node.runtime_dependencies {
            bytes.extend_from_slice(b"runtime:");
            bytes.extend_from_slice(digest_graph_path(graph, &dependency.importer).as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(dependency.specifier.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(digest_graph_path(graph, &dependency.target).as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(
                indices
                    .get(&dependency.target)
                    .copied()
                    .unwrap_or(usize::MAX)
                    .to_string()
                    .as_bytes(),
            );
            bytes.push(0);
        }
        for ((captured, specifier), message) in graph.runtime_failures(&node.path) {
            bytes.extend_from_slice(b"runtime-failure:");
            bytes.extend_from_slice(digest_graph_path(graph, &node.path).as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(digest_graph_path(graph, captured).as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(specifier.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(message.as_bytes());
            bytes.push(0);
        }
    }
    if let Some(failures) = native_runtime_failures {
        for ((owner, captured, specifier), message) in failures {
            bytes.extend_from_slice(b"native-runtime-failure:");
            bytes.extend_from_slice(digest_graph_path(graph, owner).as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(digest_graph_path(graph, captured).as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(specifier.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(message.as_bytes());
            bytes.push(0);
        }
    }
}

fn graph_topology_digest(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    native_failures: Option<&BTreeMap<(PathBuf, String), String>>,
    native_runtime_failures: Option<&BTreeMap<(PathBuf, PathBuf, String), String>>,
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
        for dependency in graph.dynamic_dependencies(&node.path) {
            bytes.extend_from_slice(b"dynamic:");
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
    for ((importer, specifier), message) in graph.dynamic_failures() {
        bytes.extend_from_slice(b"dynamic-failure:");
        bytes.extend_from_slice(importer.to_string_lossy().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(specifier.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(message.as_bytes());
        bytes.push(0);
    }
    if let Some(failures) = native_failures {
        for ((importer, specifier), message) in failures {
            bytes.extend_from_slice(b"native-dynamic-failure:");
            bytes.extend_from_slice(importer.to_string_lossy().as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(specifier.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(message.as_bytes());
            bytes.push(0);
        }
    }
    append_runtime_digest(&mut bytes, graph, modules, native_runtime_failures);
    Digest::hash(&bytes)
}

/// Identity of the graph wiring used by the shared wrapper. Local bytecode slot assignments are
/// deliberately excluded: changing a module body may renumber those private slots without
/// changing the provider/import topology or the wrapper source.
fn graph_shared_topology_digest(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    native_failures: Option<&BTreeMap<(PathBuf, String), String>>,
    native_runtime_failures: Option<&BTreeMap<(PathBuf, PathBuf, String), String>>,
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
        for dependency in graph.dynamic_dependencies(&node.path) {
            bytes.extend_from_slice(b"dynamic:");
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
    for ((importer, specifier), message) in graph.dynamic_failures() {
        bytes.extend_from_slice(b"dynamic-failure:");
        bytes.extend_from_slice(importer.to_string_lossy().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(specifier.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(message.as_bytes());
        bytes.push(0);
    }
    if let Some(failures) = native_failures {
        for ((importer, specifier), message) in failures {
            bytes.extend_from_slice(b"native-dynamic-failure:");
            bytes.extend_from_slice(importer.to_string_lossy().as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(specifier.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(message.as_bytes());
            bytes.push(0);
        }
    }
    append_runtime_digest(&mut bytes, graph, modules, native_runtime_failures);
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum NativeGraphImport {
    Internal,
    Binding {
        provider_index: usize,
        export_index: usize,
    },
    Namespace {
        provider_index: usize,
    },
    CommonJsNamed {
        provider_index: usize,
        property: String,
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
        Some(ResolvedExport::CommonJsNamed { module, property }) => {
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
                    feature: format!(
                        "`{}`, imported across a module cycle before `{}` has run, in native module graphs",
                        property,
                        module.display()
                    ),
                }));
            }
            return Ok(NativeGraphImport::CommonJsNamed {
                provider_index,
                property: property.clone(),
            });
        }
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
    live_imports: Option<&BTreeSet<(PathBuf, usize)>>,
    partial_namespace_members: Option<&BTreeMap<PathBuf, BTreeSet<String>>>,
) -> Result<Vec<Option<Vec<NativeNamespaceEntry>>>, NativeError> {
    let has_any_dynamic_import = modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import());
    let mut required = vec![false; modules.len()];
    for (node, _) in modules {
        for (import_index, import) in node.program.imports.iter().enumerate() {
            if live_imports
                .is_some_and(|imports| !imports.contains(&(node.path.clone(), import_index)))
            {
                continue;
            }
            if let NativeGraphImport::Namespace { provider_index } =
                graph_import_value(graph, modules, indices, node, import)?
            {
                required[provider_index] = true;
            }
        }
    }
    if has_any_dynamic_import {
        for (idx, (node, _)) in modules.iter().enumerate() {
            if node.source != ModuleSource::CommonJs {
                required[idx] = true;
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
        if node.source == ModuleSource::CommonJs {
            plans[module_index] = None;
            continue;
        }
        let mut names = graph.resolved_export_names(&node.path).ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::InternalMissingModule {
                path: node.path.clone(),
            })
        })?;
        if !has_any_dynamic_import {
            if let Some(members) = partial_namespace_members.and_then(|all| all.get(&node.path)) {
                names.retain(|name| members.contains(name));
            }
        }
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
                    Some(ResolvedExport::CommonJsNamed { module, .. }) => {
                        let (provider_index, export_index) =
                            graph_export_slot(modules, indices, &node.path, module, 0, &name)?;
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
    } else if let ModuleSource::NodeBuiltin(_builtin) = node.source {
        node.program.exports.len()
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
        ModuleSource::Json | ModuleSource::Asset => Some(0),
        ModuleSource::NodeBuiltin(_) => node
            .program
            .exports
            .iter()
            .position(|e| e.exported == "default"),
        ModuleSource::Source | ModuleSource::CommonJs => None,
    }
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

fn find_opentui_library(project_root: &Path) -> Option<PathBuf> {
    if let Some(env_path) = std::env::var_os("OPENTUI_LIB_PATH") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return Some(p);
        }
    }
    let standard = project_root.join("node_modules/@opentui/core-linux-x64/libopentui.so");
    if standard.exists() {
        return Some(standard);
    }
    let standard_musl =
        project_root.join("node_modules/@opentui/core-linux-x64-musl/libopentui.so");
    if standard_musl.exists() {
        return Some(standard_musl);
    }
    let bun_dir = project_root.join("node_modules/.bun");
    if bun_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&bun_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("@opentui+core-linux-x64") {
                    let candidate = entry
                        .path()
                        .join("node_modules/@opentui/core-linux-x64/libopentui.so");
                    if candidate.exists() {
                        return Some(candidate);
                    }
                    let candidate_musl = entry
                        .path()
                        .join("node_modules/@opentui/core-linux-x64-musl/libopentui.so");
                    if candidate_musl.exists() {
                        return Some(candidate_musl);
                    }
                }
            }
        }
    }
    let nm = project_root.join("node_modules");
    if nm.is_dir() {
        let mut queue = std::collections::VecDeque::new();
        queue.push_back((nm, 0usize));
        while let Some((dir, depth)) = queue.pop_front() {
            if depth > 4 {
                continue;
            }
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(file_name) = path.file_name() {
                            if file_name == "libopentui.so" || file_name == "libopentui.dylib" {
                                return Some(path);
                            }
                        }
                    } else if path.is_dir() {
                        queue.push_back((path, depth + 1));
                    }
                }
            }
        }
    }
    None
}

fn write_dynamic_failure_dispatch(
    output: &mut String,
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    indent: &str,
    native_failures: Option<&BTreeMap<(PathBuf, String), String>>,
) {
    let native_iter = native_failures.into_iter().flat_map(BTreeMap::iter);
    for ((importer_path, specifier), message) in graph.dynamic_failures().chain(native_iter) {
        let Some(importer_index) = modules
            .iter()
            .position(|(node, _)| node.path == *importer_path)
        else {
            continue;
        };
        let importer = native_c_string(&modules[importer_index].1.symbol);
        let c_spec = native_c_string(specifier);
        let c_message = native_c_string(message);
        writeln!(
            output,
            "{indent}if (importer != NULL && strcmp(importer, {importer}) == 0 && kiln_c99_string_equals_ascii(specifier, {c_spec}, (size_t){})) {{ kiln_value err = kiln_c99_undefined(); (void)kiln_c99_new_type_error(&err, {c_message}); return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown); }}",
            specifier.len()
        )
        .unwrap();
    }
}

fn write_launcher_dynamic_import(
    output: &mut String,
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    logical_paths: &[String],
    native_failures: Option<&BTreeMap<(PathBuf, String), String>>,
) {
    let opentui_path = find_opentui_library(graph.project_root());
    output.push_str(
        &r#"
typedef struct kiln_graph_context kiln_graph_context;
static kiln_value *kiln_graph_module_result(kiln_graph_context *graph, size_t index);
static int kiln_graph_ensure_module(kiln_graph_context *graph, size_t index);
static kiln_value *kiln_graph_module_promise(kiln_graph_context *graph, size_t index);
int kiln_graph_module_completion(kiln_host *host, uint32_t index, int rejected, const kiln_value *value, kiln_value *result, int depth, kiln_value *thrown);
extern int kiln_c99_module_completion(kiln_host *, kiln_value, uint32_t, kiln_value *, int, kiln_value *);
extern int kiln_c99_finish_module_checkpoint(kiln_host *, kiln_value *);
#define KILN_C99_METHOD_THEN 107
extern int kiln_c99_validate_dynamic_import_options(kiln_host *, kiln_value, int, kiln_value *, int *);
int kiln_graph_dynamic_import(kiln_host *host, const char *importer, kiln_value specifier, kiln_value options, kiln_value *promise, int depth, kiln_value *thrown) {
    if (promise == NULL) return KILN_C99_CONTROL_ERROR;
    if (specifier.tag != KILN_VALUE_STRING) {
        kiln_value primitive = kiln_c99_undefined();
        int specifier_status = kiln_c99_to_primitive(host, specifier, KILN_C99_HINT_STRING, &primitive, depth, thrown);
        if (specifier_status != KILN_C99_OK) {
            if (thrown != NULL) return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), thrown, 1, depth, thrown);
            return specifier_status;
        }
        specifier = kiln_c99_text(primitive);
    }
    if (specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) {
        kiln_value err = kiln_c99_undefined();
        (void)kiln_c99_new_type_error(&err, "invalid import specifier");
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);
    }
    int attribute_type = 0;
    { kiln_value validation_thrown = kiln_c99_undefined();
      int attr_status = kiln_c99_validate_dynamic_import_options(host, options, depth, &validation_thrown, &attribute_type);
      if (attr_status != KILN_C99_OK) {
          kiln_value *reason = attr_status == KILN_C99_THROWN ? &validation_thrown : &validation_thrown;
          return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), reason, 1, depth, thrown);
      }
    }
import_attributes_valid:
    if (kiln_c99_string_starts_ascii(specifier, "@opentui/core-", 14)) {
        if (kiln_opentui_namespace.tag != KILN_VALUE_UNDEFINED) {
            return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_opentui_namespace, 1, depth, thrown);
        }
        static const char *opentui_candidates[] = {
"#,
    );
    if let Some(path) = &opentui_path {
        writeln!(
            output,
            "            {},",
            native_c_string(&path.to_string_lossy())
        )
        .unwrap();
    }
    output.push_str(
        r#"            "./libopentui.so",
            "./libopentui.dylib",
            "./node_modules/@opentui/core-linux-x64/libopentui.so",
            "./node_modules/.bun/@opentui+core-linux-x64@0.4.2/node_modules/@opentui/core-linux-x64/libopentui.so",
            "./node_modules/.bun/@opentui+core-linux-x64-musl@0.4.2/node_modules/@opentui/core-linux-x64-musl/libopentui.so",
            "/usr/local/lib/libopentui.so",
            "/usr/lib/libopentui.so"
        };
        const char *so_path = getenv("OPENTUI_LIB_PATH");
        if (so_path != NULL && access(so_path, F_OK) != 0) {
            so_path = NULL;
        }
        if (so_path == NULL) {
            for (size_t i = 0; i < sizeof(opentui_candidates) / sizeof(opentui_candidates[0]); ++i) {
                if (opentui_candidates[i] != NULL && access(opentui_candidates[i], F_OK) == 0) {
                    so_path = opentui_candidates[i];
                    break;
                }
            }
        }
        if (so_path == NULL) {
            so_path = opentui_candidates[0];
        }
        kiln_value ns_obj;
        int status = kiln_c99_new_object(&ns_obj);
        if (status == KILN_C99_OK) {
            kiln_value def_val = kiln_c99_copy_text(so_path, strlen(so_path));
            status = kiln_c99_put(host, ns_obj, "default", 7, def_val, depth, thrown);
        }
        if (status != KILN_C99_OK) return status;
        kiln_opentui_namespace = ns_obj;
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_opentui_namespace, 1, depth, thrown);
    }
    if (kiln_c99_string_equals_ascii(specifier, "node:ffi", 8) ||
        kiln_c99_string_equals_ascii(specifier, "ffi", 3)) {
        if (kiln_node_ffi_namespace.tag != KILN_VALUE_UNDEFINED) {
            return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_node_ffi_namespace, 1, depth, thrown);
        }
        kiln_value dlopen = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 495 };
        kiln_value get_raw_pointer = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 496 };
        kiln_value to_array_buffer = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 497 };
#if defined(_WIN32)
        kiln_value suffix = kiln_c99_copy_text(".dll", 4);
#elif defined(__APPLE__)
        kiln_value suffix = kiln_c99_copy_text(".dylib", 6);
#else
        kiln_value suffix = kiln_c99_copy_text(".so", 3);
#endif
        kiln_value ns_obj;
        int status = kiln_c99_new_object(&ns_obj);
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "default", 7, ns_obj, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "dlopen", 6, dlopen, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "getRawPointer", 13, get_raw_pointer, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "suffix", 6, suffix, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "toArrayBuffer", 13, to_array_buffer, depth, thrown);
        }
        if (status != KILN_C99_OK) return status;
        kiln_node_ffi_namespace = ns_obj;
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_node_ffi_namespace, 1, depth, thrown);
    }
"#,
    );
    write_dynamic_failure_dispatch(output, graph, modules, "    ", native_failures);
    for (index, (node, _)) in modules.iter().enumerate() {
        let mut resolutions = std::collections::BTreeSet::new();
        resolutions.insert((logical_paths[index].clone(), index));
        if let Some(stripped) = logical_paths[index].strip_prefix("./") {
            resolutions.insert((stripped.to_owned(), index));
        } else {
            resolutions.insert((format!("./{}", logical_paths[index]), index));
        }
        if let ModuleSource::NodeBuiltin(builtin) = node.source {
            resolutions.insert((builtin.specifier().to_owned(), index));
            if let Some(stripped) = builtin.specifier().strip_prefix("node:") {
                resolutions.insert((stripped.to_owned(), index));
            }
        }
        for (other_index, (other_node, _)) in modules.iter().enumerate() {
            for dep in graph.dynamic_dependencies(&other_node.path) {
                if dep.target == node.path {
                    resolutions.insert((dep.specifier.clone(), other_index));
                }
            }
        }
        for (spec, importer_index) in resolutions {
            let c_spec = native_c_string(&spec);
            let len = spec.len();
            let importer = native_c_string(&modules[importer_index].1.symbol);
            let attribute_guard = if matches!(node.source, ModuleSource::Asset) {
                " && attribute_type == 1"
            } else {
                " && attribute_type == 0"
            };
            writeln!(
                output,
                "    if (importer != NULL && strcmp(importer, {importer}) == 0{attribute_guard} && kiln_c99_string_equals_ascii(specifier, {c_spec}, (size_t){len})) {{"
            )
            .unwrap();
            writeln!(
                output,
                "        int status = kiln_graph_ensure_module((kiln_graph_context *)host->context, (size_t){index});\n        if (status != KILN_C99_OK) return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), kiln_graph_module_result((kiln_graph_context *)host->context, (size_t){index}), 1, depth, thrown);\n        if (kiln_graph_module_promise((kiln_graph_context *)host->context, (size_t){index})->tag == 16 /* KILN_VALUE_PROMISE */) return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_THEN, *kiln_graph_module_promise((kiln_graph_context *)host->context, (size_t){index}), NULL, 0, depth, thrown);"
            )
            .unwrap();
            if modules[index].0.source == ModuleSource::CommonJs {
                writeln!(
                    output,
                    "        kiln_value load_res;\n        status = kiln_graph_load_commonjs(&kiln_commonjs_{index}, &load_res, thrown);\n        if (status != KILN_C99_OK) return status;\n        kiln_namespaces[{index}] = kiln_exports_{index}[0];"
                )
                .unwrap();
            }
            writeln!(
                output,
                "        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_namespaces[{index}], 1, depth, thrown);\n    }}"
            )
            .unwrap();
        }
    }
    output.push_str(
        r#"    {
        kiln_value err = kiln_c99_undefined();
        (void)kiln_c99_new_type_error(&err, "no module loader is installed");
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);
    }
}
"#,
    );
}

fn write_shared_wrapper_dynamic_import(
    output: &mut String,
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    logical_paths: &[String],
    native_failures: &BTreeMap<(PathBuf, String), String>,
) {
    let opentui_path = find_opentui_library(graph.project_root());
    output.push_str(
        r#"
extern int kiln_c99_validate_dynamic_import_options(kiln_host *, kiln_value, int, kiln_value *, int *);
int kiln_graph_dynamic_import(kiln_host *host, const char *importer, kiln_value specifier, kiln_value options, kiln_value *promise, int depth, kiln_value *thrown) {
    if (promise == NULL) return KILN_C99_CONTROL_ERROR;
    if (specifier.tag != KILN_VALUE_STRING) {
        kiln_value primitive = kiln_c99_undefined();
        int specifier_status = kiln_c99_to_primitive(host, specifier, KILN_C99_HINT_STRING, &primitive, depth, thrown);
        if (specifier_status != KILN_C99_OK) {
            if (thrown != NULL) return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), thrown, 1, depth, thrown);
            return specifier_status;
        }
        specifier = kiln_c99_text(primitive);
    }
    if (specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) {
        kiln_value err = kiln_c99_undefined();
        (void)kiln_c99_new_type_error(&err, "invalid import specifier");
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);
    }
    int attribute_type = 0;
    { kiln_value validation_thrown = kiln_c99_undefined();
      int attr_status = kiln_c99_validate_dynamic_import_options(host, options, depth, &validation_thrown, &attribute_type);
      if (attr_status != KILN_C99_OK) {
          return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &validation_thrown, 1, depth, thrown);
      }
    }
import_attributes_valid:
    if (specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) {
        kiln_value err = kiln_c99_undefined();
        (void)kiln_c99_new_type_error(&err, "invalid import specifier");
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);
    }
    if (kiln_c99_string_starts_ascii(specifier, "@opentui/core-", 14)) {
        if (kiln_shared_opentui_namespace.tag != KILN_VALUE_UNDEFINED) {
            return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_shared_opentui_namespace, 1, depth, thrown);
        }
        static const char *opentui_candidates[] = {
"#,
    );
    if let Some(path) = &opentui_path {
        writeln!(
            output,
            "            {},",
            native_c_string(&path.to_string_lossy())
        )
        .unwrap();
    }
    output.push_str(
        r#"            "./libopentui.so",
            "./libopentui.dylib",
            "./node_modules/@opentui/core-linux-x64/libopentui.so",
            "./node_modules/.bun/@opentui+core-linux-x64@0.4.2/node_modules/@opentui/core-linux-x64/libopentui.so",
            "./node_modules/.bun/@opentui+core-linux-x64-musl@0.4.2/node_modules/@opentui/core-linux-x64-musl/libopentui.so",
            "/usr/local/lib/libopentui.so",
            "/usr/lib/libopentui.so"
        };
        const char *so_path = NULL;
        for (size_t i = 0; i < sizeof(opentui_candidates) / sizeof(opentui_candidates[0]); ++i) {
            if (opentui_candidates[i] != NULL && access(opentui_candidates[i], F_OK) == 0) {
                so_path = opentui_candidates[i];
                break;
            }
        }
        if (so_path == NULL) {
            so_path = opentui_candidates[0];
        }
        kiln_value ns_obj;
        int status = kiln_c99_new_object(&ns_obj);
        if (status == KILN_C99_OK) {
            kiln_value def_val = kiln_c99_copy_text(so_path, strlen(so_path));
            status = kiln_c99_put(host, ns_obj, "default", 7, def_val, depth, thrown);
        }
        if (status != KILN_C99_OK) return status;
        kiln_shared_opentui_namespace = ns_obj;
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_shared_opentui_namespace, 1, depth, thrown);
    }
    if (kiln_c99_string_equals_ascii(specifier, "node:ffi", 8) ||
        kiln_c99_string_equals_ascii(specifier, "ffi", 3)) {
        if (kiln_shared_node_ffi_namespace.tag != KILN_VALUE_UNDEFINED) {
            return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_shared_node_ffi_namespace, 1, depth, thrown);
        }
        kiln_value dlopen = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 495 };
        kiln_value get_raw_pointer = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 496 };
        kiln_value to_array_buffer = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 497 };
#if defined(_WIN32)
        kiln_value suffix = kiln_c99_copy_text(".dll", 4);
#elif defined(__APPLE__)
        kiln_value suffix = kiln_c99_copy_text(".dylib", 6);
#else
        kiln_value suffix = kiln_c99_copy_text(".so", 3);
#endif
        kiln_value ns_obj;
        int status = kiln_c99_new_object(&ns_obj);
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "default", 7, ns_obj, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "dlopen", 6, dlopen, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "getRawPointer", 13, get_raw_pointer, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "suffix", 6, suffix, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "toArrayBuffer", 13, to_array_buffer, depth, thrown);
        }
        if (status != KILN_C99_OK) return status;
        kiln_shared_node_ffi_namespace = ns_obj;
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_shared_node_ffi_namespace, 1, depth, thrown);
    }
    if (kiln_shared_namespaces != NULL) {
"#,
    );
    write_dynamic_failure_dispatch(output, graph, modules, "        ", Some(native_failures));
    for (index, (node, _)) in modules.iter().enumerate() {
        let mut resolutions = std::collections::BTreeSet::new();
        resolutions.insert((logical_paths[index].clone(), index));
        if let Some(stripped) = logical_paths[index].strip_prefix("./") {
            resolutions.insert((stripped.to_owned(), index));
        } else {
            resolutions.insert((format!("./{}", logical_paths[index]), index));
        }
        if let ModuleSource::NodeBuiltin(builtin) = node.source {
            resolutions.insert((builtin.specifier().to_owned(), index));
            if let Some(stripped) = builtin.specifier().strip_prefix("node:") {
                resolutions.insert((stripped.to_owned(), index));
            }
        }
        for (other_index, (other_node, _)) in modules.iter().enumerate() {
            for dep in graph.dynamic_dependencies(&other_node.path) {
                if dep.target == node.path {
                    resolutions.insert((dep.specifier.clone(), other_index));
                }
            }
        }
        for (spec, importer_index) in resolutions {
            let c_spec = native_c_string(&spec);
            let len = spec.len();
            let importer = native_c_string(&modules[importer_index].1.symbol);
            let attribute_guard = if matches!(node.source, ModuleSource::Asset) {
                " && attribute_type == 1"
            } else {
                " && attribute_type == 0"
            };
            writeln!(
                output,
                "        if (importer != NULL && strcmp(importer, {importer}) == 0{attribute_guard} && kiln_c99_string_equals_ascii(specifier, {c_spec}, (size_t){len})) {{"
            )
            .unwrap();
            if modules[index].0.source == ModuleSource::CommonJs {
                writeln!(
                    output,
                    "            if (kiln_shared_commonjs != NULL) {{\n                kiln_value load_res;\n                int status = kiln_graph_load_commonjs(&kiln_shared_commonjs[{index}], &load_res, thrown);\n                if (status != KILN_C99_OK) return status;\n                kiln_shared_namespaces[{index}] = kiln_shared_commonjs[{index}].exports[0];\n            }}"
                )
                .unwrap();
            }
            writeln!(
                output,
                "            return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_shared_namespaces[{index}], 1, depth, thrown);\n        }}"
            )
            .unwrap();
        }
    }
    output.push_str(
        r#"    }
    {
        kiln_value err = kiln_c99_undefined();
        (void)kiln_c99_new_type_error(&err, "no module loader is installed");
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);
    }
}
"#,
    );
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
        "    status = {module_symbol}_commonjs_wrapper(import_arrays[{index}], (size_t)5, {}, (size_t){}, {}, (size_t){}, kiln_commonjs_require_{index}, commonjs_graph);\n    if (status != KILN_C99_OK) goto cleanup;\n    export_arrays[{index}][0] = import_arrays[{index}][0];\n    commonjs_modules[{index}] = (kiln_commonjs_context){{ {module_symbol}, proxy_host, import_arrays[{index}], (size_t)5, export_arrays[{index}], 0 }};",
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
    if modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import())
    {
        output.push_str("    (void)kiln_realm_gc_mark_value(heap, &kiln_opentui_namespace);\n");
        output.push_str("    (void)kiln_realm_gc_mark_value(heap, &kiln_node_ffi_namespace);\n");
    }
    writeln!(
        output,
        "    if (graph != NULL) {{\n        (void)kiln_realm_gc_mark_value(heap, &graph->result);\n        (void)kiln_realm_gc_mark_value(heap, &graph->entry_result);\n        for (index = 0; index < (size_t){}; ++index) {{\n            (void)kiln_realm_gc_mark_value(heap, &graph->module_states[index].result);\n            (void)kiln_realm_gc_mark_value(heap, &graph->module_states[index].active_promise);\n            (void)kiln_realm_gc_mark_value(heap, &graph->module_states[index].completion_promise);\n            (void)kiln_realm_gc_mark_value(heap, &graph->module_states[index].completion_resolve);\n            (void)kiln_realm_gc_mark_value(heap, &graph->module_states[index].completion_reject);\n        }}\n    }}\n}}\n",
        modules.len()
    )
    .unwrap();
}

#[cfg(test)]
fn graph_launcher_source(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    policy: &NativeRealmPolicy,
) -> Result<String, NativeError> {
    graph_launcher_source_with_imports(graph, modules, policy, None, None, None, None, None, None)
}

/// What a startup module's completion handler does with a rejection nobody awaits.
const FATAL_UNHANDLED_REJECTION: &str = r#"/* A startup module has no importer waiting on it, so its rejection is unhandled and, as in node, fatal. This runs as a `then` reaction, so a thrown status would only reject the derived promise; and the launcher reads the entry promise only once the turn loop is quiescent, which a listening server never lets it be -- a rejected top-level await beside a live server hung the process here. */ { kiln_cli_report_failure_in(KILN_C99_THROWN, kiln_cli_describe_error(*value), "module body"); exit(1); }"#;

fn graph_launcher_source_with_imports(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    policy: &NativeRealmPolicy,
    live_imports: Option<&BTreeSet<(PathBuf, usize)>>,
    inactive_commonjs_dependencies: Option<&BTreeSet<(PathBuf, String)>>,
    partial_namespace_members: Option<&BTreeMap<PathBuf, BTreeSet<String>>>,
    native_failures: Option<&BTreeMap<(PathBuf, String), String>>,
    native_runtime_failures: Option<&BTreeMap<(PathBuf, PathBuf, String), String>>,
    bootstrap: Option<&kiln_backend_c::CModule>,
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
    let mut runtime_importers = BTreeMap::<PathBuf, usize>::new();
    for (node, _) in modules {
        for dependency in &node.runtime_dependencies {
            let next = runtime_importers.len();
            runtime_importers
                .entry(dependency.importer.clone())
                .or_insert(next);
        }
        for ((importer, _), _) in graph.runtime_failures(&node.path) {
            let next = runtime_importers.len();
            runtime_importers.entry(importer.clone()).or_insert(next);
        }
        if let Some(failures) = native_runtime_failures {
            for ((owner, importer, _), _) in failures {
                if owner == &node.path {
                    let next = runtime_importers.len();
                    runtime_importers.entry(importer.clone()).or_insert(next);
                }
            }
        }
    }
    let mut indices = std::collections::HashMap::with_capacity(modules.len());
    for (index, (node, _)) in modules.iter().enumerate() {
        indices.insert(node.path.clone(), index);
    }
    let entry_index = modules
        .iter()
        .position(|(node, _)| node.path == graph.entry())
        .expect("entry module is present in the graph");
    // The entry is the sole startup root. Its static dependency walk recursively ensures every
    // reachable provider, while the realm turn loop drives any pending top-level-await
    // continuations. Starting every reachable module independently can evaluate a cycle from the
    // dependency side before the authoritative entry has established its state.
    let startup_indices = vec![entry_index];
    let namespace_plans = graph_namespace_plans(
        graph,
        modules,
        &indices,
        live_imports,
        partial_namespace_members,
    )?;
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

"#,
    );
    output.push_str(kiln_native_runtime::kiln_value_abi_declaration!());
    output.push_str(kiln_native_runtime::kiln_string_abi_declaration!());
    output.push_str(r#"/* The prelude owns this realm-local guard. The trusted builtin bootstrap may install methods;
 * the launcher seals those writes before any user module can observe or mutate the realm. */
extern uint64_t kiln_c99_builtin_store_epoch __attribute__((visibility("hidden")));

static kiln_value kiln_graph_diagnostic(const char *phase, const char *module) {
    kiln_value value = { .tag = KILN_VALUE_STRING, .flags = 0, .len = 0, .bits = 0 };
    static char text[2048];
    static kiln_string string = {0, KILN_STRING_LATIN1, text};
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
    string.unit_len = (uint32_t)value.len;
    string.data = text;
    value.ptr = (const char *)&string;
    return value;
}

static inline int kiln_c99_string_equals_ascii(kiln_value value, const char *ascii, size_t length) {
    if (value.tag != KILN_VALUE_STRING || value.ptr == NULL) return 0;
    return kiln_string_equals_ascii((const kiln_string *)value.ptr, ascii, length);
}

static inline int kiln_c99_string_starts_ascii(kiln_value value, const char *ascii, size_t length) {
    if (value.tag != KILN_VALUE_STRING || value.ptr == NULL) return 0;
    return kiln_string_starts_ascii((const kiln_string *)value.ptr, ascii, length);
}

typedef int (*kiln_host_call_fn)(void *context, uint16_t operation, uint16_t argc,
                                 const kiln_value *arguments, kiln_value *result);
typedef struct kiln_host {
    kiln_host_call_fn call;
    void *context;
} kiln_host;

static int kiln_c99_filename_matches(kiln_value value, const char *path, size_t path_len) {
    if (path == NULL || value.tag != KILN_VALUE_STRING || (value.len != 0 && value.ptr == NULL)) return 0;
    const kiln_string *s = (const kiln_string *)value.ptr;
    if (s == NULL) return 0;
    if (s->unit_len == path_len && (path_len == 0 || kiln_string_equals_ascii(s, path, path_len))) return 1;
    if (s->unit_len >= 7 && kiln_string_starts_ascii(s, "file://", 7) && s->unit_len - 7 == path_len) {
        if (path_len == 0) return 1;
        for (size_t i = 0; i < path_len; ++i) {
            if (kiln_string_unit_at(s, i + 7) != (uint8_t)path[i]) return 0;
        }
        return 1;
    }
    return 0;
}

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
extern void kiln_cli_warn(const char *message);
extern kiln_value kiln_cli_last_host_error;
extern kiln_value kiln_cli_describe_error(kiln_value value);
extern int kiln_c99_get(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value *out, int depth, kiln_value *thrown);
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
#define KILN_C99_THROWN (-7)
#define KILN_C99_CONTROL_ERROR (-2)
#define KILN_C99_HOST_ERROR (-3)
extern int kiln_c99_process_object(kiln_value *result);
#define KILN_C99_INVALID_ARGUMENT (-1)
#define KILN_C99_HOST_MODULE_CREATE_REQUIRE UINT16_C(0x7FF0)
#define KILN_C99_HINT_STRING 1
extern int kiln_c99_commonjs_require_value(kiln_value *out, kiln_commonjs_require_fn call, void *context);
extern int kiln_c99_default_require(void *context, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown);
extern int kiln_c99_new_object(kiln_value *result);
extern int kiln_c99_put(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value value, int depth, kiln_value *thrown);
extern kiln_value kiln_c99_copy_text(const char *text, size_t len);
extern kiln_value kiln_c99_undefined(void);
extern int kiln_c99_to_primitive(kiln_host *host, kiln_value value, int hint, kiln_value *out, int depth, kiln_value *thrown);
extern kiln_value kiln_c99_text(kiln_value value);
extern kiln_value kiln_c99_control_diagnostic(void);
extern int kiln_c99_diagnostic_available(void);
extern kiln_value kiln_c99_concat(kiln_value left, kiln_value right);
extern int kiln_c99_throw_error(kiln_value *out, uint64_t kind, const char *message);
extern int kiln_c99_new_type_error(kiln_value *out, const char *message);
extern int kiln_c99_invoke_method(kiln_host *host, kiln_value *result, uint64_t code,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *thrown);
extern int kiln_c99_call(kiln_host *host, kiln_value *result, kiln_value callee,
                         kiln_value receiver, const kiln_value *args, size_t argc,
                         int depth, kiln_value *thrown);
extern int kiln_c99_module_completion(kiln_host *, kiln_value, uint32_t, kiln_value *, int, kiln_value *);
extern int kiln_c99_finish_module_checkpoint(kiln_host *, kiln_value *);
#define KILN_C99_OK 0
#define KILN_C99_METHOD_PROMISE_RESOLVE 110
#define KILN_C99_METHOD_PROMISE_REJECT 111
#define KILN_C99_METHOD_PROMISE_WITH_RESOLVERS 411
"#,
    );
    for (node, module) in modules {
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
        if node
            .program
            .program
            .code
            .iter()
            .any(|i| matches!(i, kiln_ir::Instruction::Await | kiln_ir::Instruction::Yield))
        {
            writeln!(
                output,
                "__attribute__((weak)) void {}_publish_exports(kiln_value *exports) {{ (void)exports; }}",
                module.symbol
            )
            .unwrap();
        }
    }
    if let Some(bootstrap) = bootstrap {
        writeln!(
            output,
            "extern int {}(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count);",
            bootstrap.symbol
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
    if modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import())
    {
        output.push_str("static kiln_value kiln_opentui_namespace;\n");
        output.push_str("static kiln_value kiln_node_ffi_namespace;\n");
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

typedef struct kiln_graph_module_state {
    kiln_value result;
    kiln_value active_promise;
    kiln_value completion_promise;
    kiln_value completion_resolve;
    kiln_value completion_reject;
    size_t dependency_cursor;
    int module_settling;
    int body_started;
    int phase;
    int failure_status;
} kiln_graph_module_state;

enum { KILN_MODULE_UNINITIALIZED = 0, KILN_MODULE_EVALUATING = 1,
       KILN_MODULE_EVALUATED = 2, KILN_MODULE_FAILED = 3 };

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
        writeln!(
            output,
            "\nstatic int kiln_commonjs_require_{index}(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {{\n    (void)opaque;\n    (void)host;\n    if (result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return -1;"
        )
        .unwrap();
        for dependency in &node.dependencies {
            if inactive_commonjs_dependencies.is_some_and(|inactive| {
                inactive.contains(&(node.path.clone(), dependency.specifier.clone()))
            }) {
                continue;
            }
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
                    "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) return kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, thrown);"
                )
                .unwrap();
            } else {
                let Some(commonjs_export) =
                    native_commonjs_projection_index(modules[provider_index].0)
                else {
                    continue;
                };
                writeln!(
                    output,
                    "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ *result = kiln_exports_{provider_index}[{commonjs_export}]; return 0; }}"
                )
                .unwrap();
            }
        }
        output.push_str("    if (thrown != NULL) *thrown = specifier;\n    return -2;\n}\n");
    }
    if modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import())
    {
        write_launcher_dynamic_import(&mut output, graph, modules, &logical_paths, native_failures);
    }
    output.push_str(
        &r#"
typedef struct kiln_launcher_promise_view {
    int state;
    kiln_value value;
} kiln_launcher_promise_view;

typedef struct kiln_graph_context kiln_graph_context;
typedef struct kiln_graph_require_context {
    kiln_graph_context *graph;
    size_t base_index;
} kiln_graph_require_context;
struct kiln_graph_context {
    kiln_host host;
    kiln_value result;
    kiln_value entry_result;
    kiln_graph_module_state module_states[MODULE_COUNT];
    size_t current_module;
    int roots_registered;
    /* The entry's top-level await never settled with the loop out of work (node: warn, exit 13). */
    int unsettled_entry;
    struct kiln_graph_require_context require_contexts[{runtime_context_count}];
};

static kiln_value *kiln_graph_module_result(kiln_graph_context *graph, size_t index) {
    return &graph->module_states[index].result;
}

static kiln_value *kiln_graph_module_promise(kiln_graph_context *graph, size_t index) {
    static kiln_value undefined = { .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };
    return graph->module_states[index].completion_promise.tag == 16 /* KILN_VALUE_PROMISE */
        ? &graph->module_states[index].completion_promise
        : &undefined;
}

static int kiln_graph_prepare_completion(kiln_graph_context *graph, size_t index) {
    kiln_graph_module_state *state;
    kiln_value deferred;
    int status;
    if (graph == NULL || index >= (size_t)MODULE_COUNT) return KILN_C99_CONTROL_ERROR;
    state = &graph->module_states[index];
    if (state->completion_promise.tag == 16 /* KILN_VALUE_PROMISE */) return KILN_C99_OK;
    status = kiln_c99_invoke_method(&graph->host,
                                    &deferred,
                                    KILN_C99_METHOD_PROMISE_WITH_RESOLVERS,
                                    kiln_c99_undefined(), NULL, 0, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_get(&graph->host, deferred, "promise", 7,
                          &state->completion_promise, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_get(&graph->host, deferred, "resolve", 7,
                              &state->completion_resolve, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_get(&graph->host, deferred, "reject", 6,
                              &state->completion_reject, 0, NULL);
    return status;
}
"#
        .replace("MODULE_COUNT", &modules.len().to_string())
        .replace(
            "{runtime_context_count}",
            &runtime_importers.len().max(1).to_string(),
        ),
    );
    output.push_str(
        "static int kiln_graph_host_call(void *context, uint16_t operation, uint16_t argc, const kiln_value *arguments, kiln_value *result);\nstatic int kiln_graph_ensure_module(kiln_graph_context *graph, size_t index);\n",
    );
    output.push_str("static int kiln_graph_runtime_require(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {\n    kiln_graph_require_context *context = (kiln_graph_require_context *)opaque;\n    kiln_graph_context *graph;\n    if (context == NULL || (graph = context->graph) == NULL || result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n    (void)host;\n    switch (context->base_index) {\n");
    for (base_index, importer) in runtime_importers.keys().enumerate() {
        output.push_str(&format!("        case {base_index}:\n"));
        for node in modules.iter().map(|(node, _)| node) {
            for dependency in &node.runtime_dependencies {
                if dependency.importer != *importer {
                    continue;
                }
                let Some(&provider_index) = indices.get(&dependency.target) else {
                    continue;
                };
                let specifier = native_c_string(&dependency.specifier);
                let specifier_len = dependency.specifier.len();
                if modules[provider_index].0.source == ModuleSource::CommonJs {
                    writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) return kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, thrown);").unwrap();
                } else if let Some(projection) =
                    native_commonjs_projection_index(modules[provider_index].0)
                {
                    writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ int status = kiln_graph_ensure_module(graph, (size_t){provider_index}); if (status != KILN_C99_OK) {{ if (thrown != NULL) *thrown = graph->module_states[{provider_index}].result; return status; }} *result = kiln_exports_{provider_index}[{projection}]; return KILN_C99_OK; }}").unwrap();
                }
            }
        }
        for node in modules.iter().map(|(node, _)| node) {
            for ((failure_importer, failure_specifier), message) in
                graph.runtime_failures(&node.path)
            {
                if failure_importer != importer {
                    continue;
                }
                writeln!(
                    output,
                    "            if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ kiln_value error = kiln_c99_undefined(); (void)kiln_c99_new_type_error(&error, {}); if (thrown != NULL) *thrown = error; return KILN_C99_THROWN; }}",
                    native_c_string(failure_specifier),
                    failure_specifier.len(),
                    native_c_string(message),
                )
                .unwrap();
            }
            if let Some(failures) = native_runtime_failures {
                for ((owner, failure_importer, failure_specifier), message) in failures {
                    if owner != &node.path || failure_importer != importer {
                        continue;
                    }
                    writeln!(
                        output,
                        "            if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ kiln_value error = kiln_c99_undefined(); (void)kiln_c99_new_type_error(&error, {}); if (thrown != NULL) *thrown = error; return KILN_C99_THROWN; }}",
                        native_c_string(failure_specifier),
                        failure_specifier.len(),
                        native_c_string(message),
                    )
                    .unwrap();
                }
            }
        }
        output.push_str("            break;\n");
    }
    output.push_str("        default: break;\n    }\n    return kiln_c99_default_require(NULL, host, specifier, result, thrown);\n}\n\n");
    output.push_str(
        "static int kiln_graph_require_builtin(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {\n    kiln_graph_context *graph = (kiln_graph_context *)opaque;\n    (void)host;\n    if (graph == NULL || result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n",
    );
    for (index, (node, _)) in modules.iter().enumerate() {
        let ModuleSource::NodeBuiltin(builtin) = node.source else {
            continue;
        };
        let Some(projection) = native_commonjs_projection_index(node) else {
            continue;
        };
        let specifier = builtin.specifier();
        writeln!(
            output,
            "    if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ int status = kiln_graph_ensure_module(graph, (size_t){}); if (status != KILN_C99_OK) {{ if (thrown != NULL) *thrown = graph->module_states[{}].result; return status; }} *result = kiln_exports_{}[{}]; return KILN_C99_OK; }}",
            native_c_string(specifier), specifier.len(), index, index, index, projection
        )
        .unwrap();
    }
    output.push_str("    return kiln_c99_default_require(NULL, host, specifier, result, thrown);\n}\n\nstatic int kiln_graph_create_require(kiln_graph_context *graph, uint16_t argc, const kiln_value *arguments, kiln_value *result) {\n    if (graph == NULL || result == NULL || argc != 1 || arguments == NULL || arguments[0].tag != KILN_VALUE_STRING || (arguments[0].len != 0 && arguments[0].ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n");
    for (base_index, importer) in runtime_importers.keys().enumerate() {
        let importer = importer.to_string_lossy();
        let file_url = Url::from_file_path(importer.as_ref())
            .ok()
            .map(|url| url.to_string());
        writeln!(
            output,
            "    if (kiln_c99_filename_matches(arguments[0], {}, (size_t){}){} ) {{ graph->require_contexts[{base_index}].graph = graph; graph->require_contexts[{base_index}].base_index = (size_t){base_index}; return kiln_c99_commonjs_require_value(result, kiln_graph_runtime_require, &graph->require_contexts[{base_index}]); }}",
            native_c_string(importer.as_ref()),
            importer.len(),
            file_url.as_ref().map(|url| format!(" || kiln_c99_filename_matches(arguments[0], {}, (size_t){})", native_c_string(url), url.len())).unwrap_or_default(),
        ).unwrap();
    }
    output.push_str(
        "    return kiln_c99_commonjs_require_value(result, kiln_graph_require_builtin, graph);\n}\n\nstatic int kiln_graph_host_call(void *context, uint16_t operation, uint16_t argc, const kiln_value *arguments, kiln_value *result) {\n    kiln_graph_context *graph = (kiln_graph_context *)context;\n    if (operation == KILN_C99_HOST_MODULE_CREATE_REQUIRE) return kiln_graph_create_require(graph, argc, arguments, result);\n    return kiln_cli_host_call(context, operation, argc, arguments, result);\n}\n",
    );
    output.push_str("static const size_t kiln_graph_startup_indices[] = {");
    for (position, index) in startup_indices.iter().enumerate() {
        if position != 0 {
            output.push_str(", ");
        }
        output.push_str(&index.to_string());
    }
    output.push_str("};\n");
    render_graph_roots(&mut output, modules);
    for index in 0..modules.len() {
        writeln!(
            output,
            "static int kiln_graph_eval_module_{index}(kiln_graph_context *graph);"
        )
        .unwrap();
    }
    output.push_str(
        "static int kiln_graph_ensure_module(kiln_graph_context *graph, size_t index);\n",
    );
    output.push_str(
        r#"
static int kiln_install_native_modules(kiln_host *host) {
    kiln_value proc_obj = kiln_c99_undefined();
    kiln_value modules_obj = kiln_c99_undefined();
    int status = kiln_c99_process_object(&proc_obj);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_new_object(&modules_obj);
    if (status != KILN_C99_OK) return status;

    kiln_value net_obj = kiln_c99_undefined();
    status = kiln_c99_new_object(&net_obj);
    if (status != KILN_C99_OK) return status;
    kiln_value net_connect = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 338 /* KILN_C99_METHOD_NODE_NET_CONNECT */ };
    kiln_value net_create_server = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 268 /* KILN_C99_METHOD_NODE_NET_CREATE_SERVER */ };
    status = kiln_c99_put(host, net_obj, "connect", 7, net_connect, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, net_obj, "createConnection", 16, net_connect, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, net_obj, "createServer", 12, net_create_server, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, modules_obj, "net", 3, net_obj, 0, NULL);
    if (status != KILN_C99_OK) return status;

    kiln_value http_obj = kiln_c99_undefined();
    status = kiln_c99_new_object(&http_obj);
    if (status != KILN_C99_OK) return status;
    kiln_value http_server = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 403 /* KILN_C99_METHOD_NODE_HTTP_SERVER */ };
    kiln_value http_server_response = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 426 /* KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE */ };
    kiln_value http_create_server = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 275 /* KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER */ };
    kiln_value http_get = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 280 /* KILN_C99_METHOD_NODE_HTTP_GET */ };
    kiln_value http_request = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 281 /* KILN_C99_METHOD_NODE_HTTP_REQUEST */ };
    status = kiln_c99_put(host, http_obj, "Server", 6, http_server, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, http_obj, "ServerResponse", 14, http_server_response, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, http_obj, "createServer", 12, http_create_server, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, http_obj, "get", 3, http_get, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, http_obj, "request", 7, http_request, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, modules_obj, "http", 4, http_obj, 0, NULL);
    if (status != KILN_C99_OK) return status;

    kiln_value fsp_obj = kiln_c99_undefined();
    status = kiln_c99_new_object(&fsp_obj);
    if (status != KILN_C99_OK) return status;
    kiln_value fs_append_file = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 482 /* KILN_C99_METHOD_NODE_BUILTIN_UNMATERIALIZED */ };
    kiln_value fs_mkdir = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 507 /* KILN_C99_METHOD_NODE_FS_MKDIR */ };
    kiln_value fs_read_file = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 379 /* KILN_C99_METHOD_NODE_FS_READ_FILE */ };
    kiln_value fs_stat = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 380 /* KILN_C99_METHOD_NODE_FS_STAT */ };
    kiln_value fs_write_file = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 381 /* KILN_C99_METHOD_NODE_FS_WRITE_FILE */ };
    status = kiln_c99_put(host, fsp_obj, "appendFile", 10, fs_append_file, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fsp_obj, "mkdir", 5, fs_mkdir, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fsp_obj, "readFile", 8, fs_read_file, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fsp_obj, "stat", 4, fs_stat, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fsp_obj, "writeFile", 9, fs_write_file, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, modules_obj, "fs_promises", 11, fsp_obj, 0, NULL);
    if (status != KILN_C99_OK) return status;

    kiln_value fs_obj = kiln_c99_undefined();
    status = kiln_c99_new_object(&fs_obj);
    if (status != KILN_C99_OK) return status;
    kiln_value fs_exists_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 378 /* KILN_C99_METHOD_NODE_FS_EXISTS_SYNC */ };
    kiln_value fs_read_file_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 514 /* KILN_C99_METHOD_NODE_FS_READ_FILE_SYNC */ };
    kiln_value fs_write_file_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 515 /* KILN_C99_METHOD_NODE_FS_WRITE_FILE_SYNC */ };
    kiln_value fs_mkdir_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 516 /* KILN_C99_METHOD_NODE_FS_MKDIR_SYNC */ };
    kiln_value fs_stat_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 517 /* KILN_C99_METHOD_NODE_FS_STAT_SYNC */ };
    kiln_value fs_lstat_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 518 /* KILN_C99_METHOD_NODE_FS_LSTAT_SYNC */ };
    kiln_value fs_readdir_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 519 /* KILN_C99_METHOD_NODE_FS_READDIR_SYNC */ };
    kiln_value fs_realpath_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 520 /* KILN_C99_METHOD_NODE_FS_REALPATH_SYNC */ };
    kiln_value fs_unlink_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 521 /* KILN_C99_METHOD_NODE_FS_UNLINK_SYNC */ };
    kiln_value fs_rmdir_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 522 /* KILN_C99_METHOD_NODE_FS_RMDIR_SYNC */ };
    status = kiln_c99_put(host, fs_obj, "existsSync", 10, fs_exists_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "readFileSync", 12, fs_read_file_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "writeFileSync", 13, fs_write_file_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "mkdirSync", 9, fs_mkdir_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "statSync", 8, fs_stat_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "lstatSync", 9, fs_lstat_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "readdirSync", 11, fs_readdir_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "realpathSync", 12, fs_realpath_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "unlinkSync", 10, fs_unlink_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "rmdirSync", 9, fs_rmdir_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "promises", 8, fsp_obj, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, modules_obj, "fs", 2, fs_obj, 0, NULL);
    if (status != KILN_C99_OK) return status;

    kiln_value ffi_obj = kiln_c99_undefined();
    status = kiln_c99_new_object(&ffi_obj);
    if (status != KILN_C99_OK) return status;
    kiln_value ffi_dlopen = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 495 /* KILN_C99_METHOD_NODE_FFI_DLOPEN */ };
    kiln_value ffi_get_raw_pointer = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 496 /* KILN_C99_METHOD_NODE_FFI_GET_RAW_POINTER */ };
    kiln_value ffi_to_array_buffer = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 497 /* KILN_C99_METHOD_NODE_FFI_TO_ARRAY_BUFFER */ };
#if defined(_WIN32)
    kiln_value ffi_suffix = kiln_c99_copy_text(".dll", 4);
#elif defined(__APPLE__)
    kiln_value ffi_suffix = kiln_c99_copy_text(".dylib", 6);
#else
    kiln_value ffi_suffix = kiln_c99_copy_text(".so", 3);
#endif
    status = kiln_c99_put(host, ffi_obj, "dlopen", 6, ffi_dlopen, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, ffi_obj, "getRawPointer", 13, ffi_get_raw_pointer, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, ffi_obj, "toArrayBuffer", 13, ffi_to_array_buffer, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, ffi_obj, "suffix", 6, ffi_suffix, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, modules_obj, "ffi", 3, ffi_obj, 0, NULL);
    if (status != KILN_C99_OK) return status;

    status = kiln_c99_put(host, proc_obj, "__kilnNativeModules", 19, modules_obj, 0, NULL);
    return status;
}

static int kiln_graph_enter(void *opaque) {
    kiln_graph_context *graph = (kiln_graph_context *)opaque;
    kiln_graph_entry_context context;
    kiln_host *host;
    kiln_value *result;
    int status = 0;
    if (graph == NULL) return -1;
    host = &graph->host;
    result = &graph->result;
    (void)context;
"#,
    );
    for base_index in 0..runtime_importers.len() {
        writeln!(
            output,
            "    graph->require_contexts[{base_index}].graph = graph; graph->require_contexts[{base_index}].base_index = (size_t){base_index};"
        )
        .unwrap();
    }
    output.push_str(
        r#"
    if (!graph->roots_registered) {
        status = kiln_realm_gc_register_roots(kiln_graph_gc_roots, graph, KILN_GC_ROOT_SURVIVES_TEARDOWN);
        if (status != 0) return status;
        graph->roots_registered = 1;
        status = kiln_install_native_modules(host);
        if (status != 0) return status;
"#,
    );
    if let Some(bootstrap) = bootstrap {
        writeln!(
            output,
            "        status = {}(host, result, NULL, 0, NULL, 0);\n        if (status != 0) {{ if (result->tag == 9 /* KILN_VALUE_OBJECT */) *result = kiln_cli_describe_error(*result); if (result->tag != KILN_VALUE_STRING || result->len == 0) *result = kiln_graph_diagnostic(\"builtin-bootstrap\", \"@kiln/runtime-bootstrap\"); return status; }}\n        kiln_c99_builtin_store_epoch = 0;",
            bootstrap.symbol
        )
        .unwrap();
    }
    for (index, (node, module)) in modules.iter().enumerate() {
        if node.source == ModuleSource::CommonJs {
            let paths = CommonJsModulePaths::new(graph.project_root(), &node.path)?;
            write_graph_commonjs_realm_setup(
                &mut output,
                index,
                &module.symbol,
                &paths.filename,
                &paths.dirname,
                &paths.diagnostic,
            );
        }
    }
    output.push_str("    }\n");
    writeln!(
        evaluation,
        "    while (graph->current_module < (size_t){}) {{\n        switch (graph->current_module) {{",
        startup_indices.len()
    )
    .unwrap();
    let mut helper_defs = String::new();
    // Single Rust-level emission point for a module case; generated C remains unchanged.
    let mut emit_module_case = |index: usize,
                                node: &kiln_module_graph::ModuleNode,
                                module: &kiln_backend_c::CModule|
     -> Result<(), NativeError> {
        let mut evaluation = &mut helper_defs;
        let is_async = node
            .program
            .program
            .code
            .iter()
            .any(|i| matches!(i, kiln_ir::Instruction::Await | kiln_ir::Instruction::Yield));
        writeln!(
            evaluation,
            "static int kiln_graph_eval_module_{index}(kiln_graph_context *graph) {{\n    kiln_graph_entry_context context;\n    kiln_graph_module_state *state = &graph->module_states[{index}];\n    kiln_host *host = &graph->host;\n    kiln_value *result = &state->result;\n    kiln_value continuation_promise = kiln_c99_undefined();\n    int status = 0;"
        )
        .unwrap();
        if !is_async {
            evaluation.push_str("    if (state->phase == KILN_MODULE_EVALUATED) return 0;\n    state->phase = KILN_MODULE_EVALUATING;\n");
        }
        let export_count = native_graph_export_count(node);
        let exports = if export_count == 0 {
            "NULL".to_owned()
        } else {
            format!("kiln_exports_{index}")
        };
        if node.source == ModuleSource::CommonJs {
            writeln!(
                evaluation,
                "            status = kiln_graph_load_commonjs(&kiln_commonjs_{index}, result, result);\n            if (status != 0) {{ if (status != KILN_C99_THROWN && result->tag == KILN_VALUE_UNDEFINED) {{ if (status == KILN_C99_HOST_ERROR && kiln_cli_last_host_error.tag == KILN_VALUE_STRING && kiln_cli_last_host_error.len != 0) *result = kiln_cli_last_host_error; else *result = kiln_graph_diagnostic(\"commonjs-evaluate\", {}); }} return status; }}",
                native_c_string(&logical_paths[index])
            )
            .unwrap();
            if node.path == graph.entry() {
                evaluation.push_str("            graph->entry_result = *result;\n");
            }
            writeln!(
                evaluation,
                "            kiln_namespaces[{index}] = kiln_exports_{index}[0];"
            )
            .unwrap();
            if let Some(plan) = &namespace_plans[index] {
                write_graph_namespace_build(&mut evaluation, index, &module.symbol, plan, false);
                writeln!(
                    evaluation,
                    "            if (status != 0) {{ *result = kiln_graph_diagnostic(\"namespace\", {}); return status; }}",
                    native_c_string(&logical_paths[index])
                )
                .unwrap();
            }
            if !is_async {
                evaluation.push_str("    state->phase = KILN_MODULE_EVALUATED;\n");
            }
            evaluation.push_str("    return 0;\n}\n");
            return Ok(());
        }

        if is_async {
            evaluation.push_str("    if (state->phase == KILN_MODULE_EVALUATED) return 0;\n    if (state->phase == KILN_MODULE_FAILED) return state->failure_status;\n");
            writeln!(
                evaluation,
                r#"            if (state->module_settling) {{
                kiln_launcher_promise_view *promise = (kiln_launcher_promise_view *)(uintptr_t)state->active_promise.bits;
                if (promise != NULL) {{
                    if (promise->state == 2 /* KILN_C99_REJECTED */) {{
                        *result = promise->value;
                        return KILN_C99_THROWN;
                    }}
                    if (promise->state == 0 /* KILN_C99_PENDING */) {{
                        *result = kiln_graph_diagnostic("top-level await", "unsettled");
                        {mark_unsettled}return KILN_C99_CONTROL_ERROR;
                    }}
                }}
                {}_publish_exports({exports});
                state->phase = KILN_MODULE_EVALUATED;"#,
                module.symbol,
                mark_unsettled = if node.path == graph.entry() {
                    "graph->unsettled_entry = 1;\n                        "
                } else {
                    ""
                }
            )
            .unwrap();
            if node.path == graph.entry() {
                evaluation.push_str("                graph->entry_result = (promise != NULL) ? promise->value : *result;\n");
            }
            if let Some(plan) = &namespace_plans[index] {
                write_graph_namespace_build(&mut evaluation, index, &module.symbol, plan, false);
                writeln!(
                    evaluation,
                    "                if (status != 0) {{ *result = kiln_graph_diagnostic(\"namespace\", {}); return status; }}",
                    native_c_string(&logical_paths[index])
                )
                .unwrap();
            }
            evaluation.push_str(
                r#"                state->module_settling = 0;
                return 0;
            }
"#,
            );
        }

        let static_dependencies: Vec<usize> = node
            .dependencies
            .iter()
            .filter(|dependency| {
                node.program
                    .dependencies
                    .iter()
                    .any(|specifier| specifier == &dependency.specifier)
            })
            .filter_map(|dependency| indices.get(&dependency.target).copied())
            .collect();
        for (dependency_cursor, provider_index) in static_dependencies.iter().enumerate() {
            writeln!(
                evaluation,
                "            if (state->dependency_cursor == (size_t){dependency_cursor}) {{ status = kiln_graph_ensure_module(graph, (size_t){provider_index}); if (status != KILN_C99_OK) return status; state->dependency_cursor++; if (kiln_graph_module_promise(graph, (size_t){provider_index})->tag == 16 /* KILN_VALUE_PROMISE */) {{ status = kiln_graph_prepare_completion(graph, (size_t){index}); if (status != KILN_C99_OK) return status; status = kiln_c99_module_completion(host, *kiln_graph_module_promise(graph, (size_t){provider_index}), (uint32_t){index}, &continuation_promise, 1, NULL); if (status != KILN_C99_OK) return status; state->module_settling = 1; return 0; }} }}"
            )
            .unwrap();
        }

        for (import_index, import) in node.program.imports.iter().enumerate() {
            if live_imports
                .is_some_and(|imports| !imports.contains(&(node.path.clone(), import_index)))
            {
                continue;
            }
            match graph_import_value(graph, modules, &indices, node, import)? {
                NativeGraphImport::Internal => {}
                NativeGraphImport::Binding {
                    provider_index,
                    export_index,
                } => {
                    if modules[provider_index].0.source == ModuleSource::CommonJs {
                        writeln!(
                            evaluation,
                            "            status = kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, result);\n            if (status != 0) return status;"
                        )
                        .unwrap();
                    }
                    writeln!(
                        evaluation,
                        "            kiln_imports_{index}[{import_index}] = kiln_exports_{provider_index}[{export_index}];"
                    )
                    .unwrap();
                }
                NativeGraphImport::Namespace { provider_index } => {
                    if modules[provider_index].0.source == ModuleSource::CommonJs {
                        writeln!(
                            evaluation,
                            "            status = kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, result);\n            if (status != 0) return status;\n            kiln_namespaces[{provider_index}] = kiln_exports_{provider_index}[0];"
                        )
                        .unwrap();
                    }
                    writeln!(
                        evaluation,
                        "            kiln_imports_{index}[{import_index}] = kiln_namespaces[{provider_index}];"
                    )
                    .unwrap();
                }
                NativeGraphImport::CommonJsNamed {
                    provider_index,
                    property,
                } => {
                    writeln!(
                        evaluation,
                        "            status = kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, result);\n            if (status != 0) return status;"
                    )
                    .unwrap();
                    let prop_c = native_c_string(&property);
                    let prop_len = property.len();
                    writeln!(
                        evaluation,
                        "            status = kiln_c99_get(host, kiln_exports_{provider_index}[0], {prop_c}, (size_t){prop_len}, &kiln_imports_{index}[{import_index}], 0, result);\n            if (status != 0) return status;"
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
        writeln!(
            evaluation,
            "            context.module = {};",
            module.symbol
        )
        .unwrap();
        evaluation
            .push_str("            context.host = host;\n            context.result = result;\n");
        writeln!(evaluation, "            context.imports = {imports};").unwrap();
        writeln!(
            evaluation,
            "            context.import_count = (size_t){};",
            node.program.imports.len()
        )
        .unwrap();
        writeln!(evaluation, "            context.exports = {exports};").unwrap();
        writeln!(
            evaluation,
            "            context.export_count = (size_t){};",
            export_count
        )
        .unwrap();
        evaluation.push_str(
            "            state->body_started = 1;\n            status = context.module(context.host, context.result, context.imports, context.import_count, context.exports, context.export_count);\n",
        );
        if node.path == graph.entry() && !is_async {
            // The whole graph evaluates inside one job (a dependency's body runs synchronously
            // before the entry's), so the checkpoint that follows is the entry's, once. An entry
            // with a top-level await takes it below, once its completion is attached.
            evaluation.push_str(
                "            if (status == 0) status = kiln_c99_finish_module_checkpoint(host, result);\n",
            );
        }
        writeln!(
            evaluation,
            "            if (status != 0) {{ if (status != KILN_C99_THROWN && result->tag == KILN_VALUE_UNDEFINED) {{ if (status == KILN_C99_HOST_ERROR && kiln_cli_last_host_error.tag == KILN_VALUE_STRING && kiln_cli_last_host_error.len != 0) *result = kiln_cli_last_host_error; else *result = kiln_graph_diagnostic(\"module-enter\", {}); }} return status; }}",
            native_c_string(&logical_paths[index])
        )
        .unwrap();
        if is_async {
            writeln!(
                evaluation,
                r#"            if (result->tag == 16 /* KILN_VALUE_PROMISE */) {{
                kiln_launcher_promise_view *promise = (kiln_launcher_promise_view *)(uintptr_t)result->bits;
                if (promise != NULL && promise->state == 0 /* KILN_C99_PENDING */) {{
                    state->active_promise = *result;
                    status = kiln_graph_prepare_completion(graph, (size_t){index});
                    if (status != KILN_C99_OK) return status;
                    status = kiln_c99_module_completion(host, *result, (uint32_t){index}, &continuation_promise, 1, NULL);
                    if (status != KILN_C99_OK) return status;
                    state->module_settling = 1;
                    {checkpoint}return 0;
                }}
                if (promise != NULL && promise->state == 2 /* KILN_C99_REJECTED */) {{
                    *result = promise->value;
                    return KILN_C99_THROWN;
                }}
                {}_publish_exports({exports});
                state->phase = KILN_MODULE_EVALUATED;
            }}"#,
                module.symbol,
                // The entry's body suspended inside the evaluation job; the checkpoint that
                // follows -- the promise queue before the tick lane -- runs once its completion
                // is attached, so a rejection of the module promise is seen the ordinary way.
                checkpoint = if node.path == graph.entry() {
                    "status = kiln_c99_finish_module_checkpoint(host, result);\n                    if (status != KILN_C99_OK) return status;\n                    "
                } else {
                    ""
                }
            )
            .unwrap();
        }
        if node.path == graph.entry() {
            if is_async {
                evaluation.push_str(
                    r#"            if (result->tag == 16) {
                kiln_launcher_promise_view *promise = (kiln_launcher_promise_view *)(uintptr_t)result->bits;
                graph->entry_result = (promise != NULL && promise->state == 1) ? promise->value : *result;
            } else {
                graph->entry_result = *result;
            }
"#,
                );
            } else {
                evaluation.push_str("            graph->entry_result = *result;\n");
            }
        }
        if let Some(plan) = &namespace_plans[index] {
            write_graph_namespace_build(&mut evaluation, index, &module.symbol, plan, false);
            writeln!(
                evaluation,
                "            if (status != 0) {{ *result = kiln_graph_diagnostic(\"namespace\", {}); return status; }}",
                native_c_string(&logical_paths[index])
            )
            .unwrap();
        }
        if !is_async {
            evaluation.push_str("    state->phase = KILN_MODULE_EVALUATED;\n");
        }
        evaluation.push_str("    if (state->completion_resolve.tag != KILN_VALUE_UNDEFINED) { kiln_value settled = kiln_c99_undefined(); status = kiln_c99_call(host, &settled, state->completion_resolve, kiln_c99_undefined(), &kiln_namespaces[");
        evaluation.push_str(&index.to_string());
        evaluation.push_str("], 1, 0, NULL); if (status != KILN_C99_OK) return status; }\n");
        evaluation.push_str("    return 0;\n}\n");
        Ok(())
    };
    for (index, (node, module)) in modules.iter().enumerate() {
        emit_module_case(index, node, module)?;
    }
    helper_defs.push_str("int kiln_graph_module_completion(kiln_host *host, uint32_t index, int rejected, const kiln_value *value, kiln_value *result, int depth, kiln_value *thrown) {\n    kiln_graph_context *graph = (kiln_graph_context *)host->context;\n    int status = KILN_C99_OK;\n    (void)depth; (void)thrown;\n    if (graph == NULL || result == NULL || value == NULL) return KILN_C99_CONTROL_ERROR;\n    switch (index) {\n");
    for (index, (node, module)) in modules.iter().enumerate() {
        let fatal = if startup_indices.contains(&index) {
            FATAL_UNHANDLED_REJECTION
        } else {
            ""
        };
        helper_defs.push_str(&format!(
            "        case {index}: {{\n            kiln_graph_module_state *state = &graph->module_states[{index}];\n            kiln_value settled = kiln_c99_undefined();\n            if (rejected) {{ state->result = *value; state->phase = KILN_MODULE_FAILED; state->failure_status = KILN_C99_THROWN; state->module_settling = 0; *result = *value; if (state->completion_reject.tag != KILN_VALUE_UNDEFINED) {{ status = kiln_c99_call(host, &settled, state->completion_reject, kiln_c99_undefined(), value, 1, depth, thrown); if (status != KILN_C99_OK) return status; }} {fatal} return KILN_C99_OK; }}\n            if (!state->body_started) {{ state->module_settling = 0; status = kiln_graph_eval_module_{index}(graph); if (status != KILN_C99_OK) {{ state->phase = KILN_MODULE_FAILED; state->failure_status = status; *result = state->result; if (state->completion_reject.tag != KILN_VALUE_UNDEFINED) {{ status = kiln_c99_call(host, &settled, state->completion_reject, kiln_c99_undefined(), &state->result, 1, depth, thrown); if (status != KILN_C99_OK) return status; }} return KILN_C99_OK; }} *result = kiln_c99_undefined(); return KILN_C99_OK; }}\n            if (state->phase == KILN_MODULE_EVALUATED) {{ *result = kiln_namespaces[{index}]; return KILN_C99_OK; }}\n",
            index = index,
            fatal = fatal
        ));
        let is_async = node
            .program
            .program
            .code
            .iter()
            .any(|i| matches!(i, kiln_ir::Instruction::Await | kiln_ir::Instruction::Yield));
        if is_async && native_graph_export_count(node) != 0 {
            helper_defs.push_str(&format!(
                "            {symbol}_publish_exports(kiln_exports_{index});\n",
                symbol = module.symbol,
                index = index
            ));
        }
        if let Some(plan) = &namespace_plans[index] {
            write_graph_namespace_build(&mut helper_defs, index, &module.symbol, plan, false);
        }
        helper_defs.push_str(&format!("            state->result = kiln_namespaces[{index}];\n            state->phase = KILN_MODULE_EVALUATED;\n            state->module_settling = 0;\n            *result = state->result;\n            if (state->completion_resolve.tag != KILN_VALUE_UNDEFINED) {{ status = kiln_c99_call(host, &settled, state->completion_resolve, kiln_c99_undefined(), &state->result, 1, depth, thrown); if (status != KILN_C99_OK) return status; }}\n", index = index));
        if node.path == graph.entry() {
            helper_defs.push_str("            graph->entry_result = *value;\n");
        }
        helper_defs.push_str("            return KILN_C99_OK;\n        }\n");
        let _ = node;
    }
    helper_defs.push_str("        default: return KILN_C99_CONTROL_ERROR;\n    }\n}\n");
    for (startup_order, &index) in startup_indices.iter().enumerate() {
        let is_async = modules[index]
            .0
            .program
            .program
            .code
            .iter()
            .any(|i| matches!(i, kiln_ir::Instruction::Await | kiln_ir::Instruction::Yield));
        if is_async {
            writeln!(evaluation, "        case {startup_order}: {{ status = kiln_graph_eval_module_{index}(graph); graph->result = graph->module_states[{index}].result; if (status != 0) return status; if (!graph->module_states[{index}].module_settling) graph->current_module++; return 0; }}").unwrap();
        } else {
            writeln!(evaluation, "        case {startup_order}: {{ status = kiln_graph_ensure_module(graph, (size_t){index}); graph->result = graph->module_states[{index}].result; if (status != 0) return status; graph->current_module++; return 0; }}").unwrap();
        }
    }
    evaluation.push_str(
        r#"        default:
            return 0;
        }
    }
"#,
    );
    output.push_str(&evaluation);
    output.push_str("    return 0;\n}\n");
    output.push_str(&helper_defs);
    output.push_str(
        "static int kiln_graph_ensure_module(kiln_graph_context *graph, size_t index) {\n",
    );
    output.push_str("    int status;\n");
    output.push_str("    if (graph == NULL || index >= (size_t)");
    output.push_str(&modules.len().to_string());
    output.push_str(
        ") return KILN_C99_CONTROL_ERROR;\n    if (graph->module_states[index].phase == KILN_MODULE_EVALUATED) return 0;\n    if (graph->module_states[index].phase == KILN_MODULE_FAILED) return graph->module_states[index].failure_status;\n    if (graph->module_states[index].phase == KILN_MODULE_EVALUATING) return 0;\n    graph->module_states[index].phase = KILN_MODULE_EVALUATING;\n    switch (index) {\n",
    );
    for index in 0..modules.len() {
        writeln!(output, "        case {index}: {{").unwrap();
        for (dependency_index, dependency) in modules[index].0.dependencies.iter().enumerate() {
            if let Some(&provider) = indices.get(&dependency.target) {
                writeln!(
                    output,
                    "            {}status = kiln_graph_ensure_module(graph, (size_t){provider}); if (status != KILN_C99_OK) {{ graph->module_states[index].result = graph->module_states[{provider}].result; graph->module_states[index].phase = KILN_MODULE_FAILED; graph->module_states[index].failure_status = status; return status; }}",
                    if dependency_index == 0 { "int " } else { "" }
                )
                .unwrap();
            }
        }
        writeln!(
            output,
            "            status = kiln_graph_eval_module_{index}(graph);\n            if (status != KILN_C99_OK) {{ graph->module_states[index].phase = KILN_MODULE_FAILED; graph->module_states[index].failure_status = status; return status; }}\n            return 0;\n        }}"
        )
        .unwrap();
    }
    output.push_str("        default: return KILN_C99_CONTROL_ERROR;\n    }\n}\n");
    output.push_str(
        r#"/* argc/argv reach the realm before it is entered, so the first `process` a module builds
 * already has the command line. The strings are copied on the other side of this call. */
static int kiln_cli_run_realm(int argc, char **argv, int writes_result) {
    kiln_graph_context graph = {0};
    uint64_t realm = 0;
    /* A refusal that reaches here has no diagnostic of its own -- the realm stopped before it
     * could leave one -- so the launcher says which of the three phases was running. */
    const char *phase = "realm creation";
    int status;
    int destroy_status;
    int unsettled_entry = 0;
    graph.result.tag = KILN_VALUE_UNDEFINED;
    graph.entry_result.tag = KILN_VALUE_UNDEFINED;
    graph.host.call = kiln_graph_host_call;
    graph.host.context = &graph;
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
    writeln!(
        output,
        r#"    while (status == 0 && (graph.current_module < (size_t){} || !graph.roots_registered)) {{
        if (graph.current_module < (size_t){} && graph.module_states[kiln_graph_startup_indices[graph.current_module]].module_settling) {{
            phase = "the turn loop";
            status = kiln_realm_drive(realm, &graph.host, &graph.result);
            if (status != 0) break;
        }}
        phase = "module body";
        status = kiln_realm_enter(realm, kiln_graph_enter, &graph);
    }}
        if (status == 0) {{ phase = "the turn loop"; status = kiln_realm_drive(realm, &graph.host, &graph.result); }}"#,
        startup_indices.len(),
        startup_indices.len()
    )
    .unwrap();
    output.push_str(
        r##"
    if (status == 0 && graph.entry_result.tag == 16 /* KILN_VALUE_PROMISE */) {
        typedef struct kiln_launcher_promise_view {
            int state;
            kiln_value value;
        } kiln_launcher_promise_view;
        kiln_launcher_promise_view *promise = (kiln_launcher_promise_view *)(uintptr_t)graph.entry_result.bits;
        if (promise != NULL) {
            if (promise->state == 2 /* KILN_C99_REJECTED */) {
                status = KILN_C99_THROWN;
                phase = "module body";
                graph.result = kiln_cli_describe_error(promise->value);
            } else if (promise->state == 0 /* KILN_C99_PENDING */) {
                status = KILN_C99_CONTROL_ERROR;
                phase = "the turn loop";
                unsettled_entry = 1;
                graph.result = kiln_graph_diagnostic("top-level await", "unsettled");
            } else if (promise->state == 1 /* KILN_C99_FULFILLED */) {
                graph.entry_result = promise->value;
            }
        }
    }
    /* Graph module entries carry the original thrown value across the module ABI so dynamic
     * import and catch preserve language identity. Only now, after the graph and its turn loop
     * have established that the value is uncaught, attach the native refusal provenance. A
     * handled throw resets the realm diagnostic bridge, so this cannot leak an earlier refusal
     * into a later ordinary error. */
    if (status == KILN_C99_THROWN) {
        kiln_value message = kiln_cli_describe_error(graph.result);
        if (message.tag != KILN_VALUE_STRING) {
            message = kiln_c99_text(message);
        }
        kiln_value site = kiln_c99_control_diagnostic();
        if (kiln_c99_diagnostic_available() && message.tag == KILN_VALUE_STRING &&
            site.tag == KILN_VALUE_STRING && site.len != 0) {
            kiln_value separator = kiln_c99_copy_text(" at ", 4);
            graph.result = kiln_c99_concat(kiln_c99_concat(message, separator), site);
        } else {
            graph.result = message;
        }
    }
    /* An entry whose top-level await never settled, with the loop out of work: node's answer is a
     * warning and status 13 rather than a refusal, and both engines give that one. */
    unsettled_entry = status == KILN_C99_CONTROL_ERROR && (unsettled_entry || graph.unsettled_entry);
    if (unsettled_entry) kiln_cli_warn("Warning: Detected unsettled top-level await");
    else if (status != 0) kiln_cli_report_failure_in(status, graph.result, phase);
    if (writes_result && status == 0 && kiln_cli_write_result(graph.entry_result) != 0) {
        static const kiln_string refusal_str = { 17, 0, "filesystem access" };
        kiln_cli_report_failure_in(KILN_C99_HOST_ERROR, (kiln_value){ .tag = KILN_VALUE_STRING, .flags = 0, .len = 17, .ptr = (const char *)&refusal_str }, "result write");
        status = 1;
    }
    destroy_status = kiln_realm_destroy(realm);
    /* `main` folds every failure to 1, the way it does a refusal; this is the one status a
     * program's caller is told apart by, as `process.exit(code)` is. */
    if (unsettled_entry) exit(13);
    if (status == 0) status = destroy_status;
    return status == 0 ? 0 : 1;
}

/* How many realms to run, read from the environment rather than fixed at build time.
 *
 * A server binds with SO_REUSEPORT, so N of these share a port and the kernel spreads the accepts
 * across them. Reading the count at startup keeps one artifact correct on every machine it is
 * copied to, which is the point of shipping a binary; baking it in at `kiln build` would tie the
 * artifact to the box that built it.
 *
 * "auto" (case-insensitive) queries online CPU cores via sysconf(_SC_NPROCESSORS_ONLN).
 * Absent, unparseable or below one means one realm -- exactly the behaviour before this existed. */
static long kiln_cli_worker_count(void) {
    const char *setting = getenv("KILN_WORKERS");
    char *end = NULL;
    long count;
    if (setting == NULL || *setting == '\0') return 1;
    if ((setting[0] == 'a' || setting[0] == 'A') &&
        (setting[1] == 'u' || setting[1] == 'U') &&
        (setting[2] == 't' || setting[2] == 'T') &&
        (setting[3] == 'o' || setting[3] == 'O') &&
        setting[4] == '\0') {
#ifndef _WIN32
        long cpus = sysconf(_SC_NPROCESSORS_ONLN);
        count = cpus > 0 ? cpus : 1;
#else
        count = 1;
#endif
    } else {
        count = strtol(setting, &end, 10);
        if (end == setting || *end != '\0' || count < 1) return 1;
    }
    if (count < 1) count = 1;
    if (count > 1024) count = 1024;
    return count;
}

/* Whether HTTP/3 support is enabled via KILN_HTTP3=1 or --http3 flag. */
#ifndef _WIN32
extern int setenv(const char *name, const char *value, int overwrite);
#endif
static int kiln_cli_http3_enabled(int argc, char **argv) {
    const char *setting = getenv("KILN_HTTP3");
    int i;
    if (setting != NULL && *setting != '\0' && strcmp(setting, "0") != 0) return 1;
    for (i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--http3") == 0) return 1;
    }
    return 0;
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
    if (kiln_cli_http3_enabled(argc, argv)) {
#ifndef _WIN32
        setenv("KILN_HTTP3", "1", 1);
#else
        _putenv("KILN_HTTP3=1");
#endif
    }
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
    Ok(output.replace(
        "UINT16_C(0x7FF0)",
        &format!("UINT16_C(0x{:04X})", C99_HOST_MODULE_CREATE_REQUIRE),
    ))
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
    kiln_string *copy;
    kiln_value normalized_small[16];
    kiln_value *normalized = NULL;
    int status;
    if (proxy == NULL || proxy->upstream == NULL || proxy->upstream->call == NULL ||
        result == NULL) return KILN_C99_HOST_ERROR;
    upstream = proxy->upstream;
    if (argc != 0) {{
        if (arguments == NULL) return KILN_C99_INVALID_ARGUMENT;
        normalized = argc <= 16 ? normalized_small :
            (kiln_value *)malloc((size_t)argc * sizeof(*normalized));
        if (normalized == NULL) return KILN_C99_HOST_ERROR;
        memcpy(normalized, arguments, (size_t)argc * sizeof(*normalized));
        for (uint16_t index = 0; index < argc; ++index) normalized[index].flags = 0;
    }}
    status = upstream->call(upstream->context, operation, argc, normalized, result);
    if (normalized != normalized_small) free(normalized);
    result->flags = 0;
    if (status != KILN_C99_OK) return status;
    if (result->tag > KILN_VALUE_STRING) {{
        proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
        return KILN_C99_INVALID_ARGUMENT;
    }}
    if (result->tag != KILN_VALUE_STRING) return KILN_C99_OK;
    if (result->len == (uint32_t)-1 || (result->len != 0 && result->ptr == NULL)) {{
        proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
        return KILN_C99_INVALID_ARGUMENT;
    }}
    {{
        const kiln_string *src_str = (const kiln_string *)result->ptr;
        size_t storage_size = 0;
        uint32_t flags = src_str != NULL ? src_str->flags : KILN_STRING_LATIN1;
        uint32_t unit_len = (uint32_t)result->len;
        if (!kiln_string_storage_size(unit_len, flags, &storage_size)) {{
            proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
            return KILN_C99_INVALID_ARGUMENT;
        }}
        copy = (kiln_string *)malloc(storage_size);
        if (copy == NULL) {{
            proxy->failure_status = KILN_C99_HOST_ERROR;
            return KILN_C99_HOST_ERROR;
        }}
        copy->unit_len = unit_len;
        copy->flags = flags;
        copy->data = (const void *)(copy + 1);
        size_t unit_bytes = (size_t)unit_len * (flags == KILN_STRING_UTF16 ? sizeof(uint16_t) : sizeof(uint8_t));
        if (unit_bytes != 0 && src_str != NULL && src_str->data != NULL) {{
            memcpy((void *)copy->data, src_str->data, unit_bytes);
        }}
        status = {arena}_track(proxy->arena, copy);
        if (status != KILN_C99_OK) {{
            free(copy);
            proxy->failure_status = KILN_C99_HOST_ERROR;
            return KILN_C99_HOST_ERROR;
        }}
        result->ptr = (const char *)copy;
        result->flags = 0;
        result->len = unit_len;
        return KILN_C99_OK;
    }}
}}

static int {run}(kiln_host *host, kiln_result_v2 **out) {{
    {arena} arena = {{ NULL, 0, 0 }};
    {proxy} proxy = {{ host, &arena, KILN_C99_OK }};
    kiln_host proxy_host = {{ {stem}_proxy_call, &proxy }};
    kiln_value value = {{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};
    kiln_result_v2 *owned = NULL;
    kiln_string *copy = NULL;
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
        (value.len == (uint32_t)-1 || (value.len != 0 && value.ptr == NULL))) {{
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
        const kiln_string *src_str = (const kiln_string *)value.ptr;
        size_t storage_size = 0;
        uint32_t flags = src_str != NULL ? src_str->flags : KILN_STRING_LATIN1;
        uint32_t unit_len = (uint32_t)value.len;
        if (!kiln_string_storage_size(unit_len, flags, &storage_size)) {{
            status = KILN_C99_INVALID_ARGUMENT;
            goto cleanup;
        }}
        copy = (kiln_string *)malloc(storage_size);
        if (copy == NULL) {{
            status = KILN_C99_HOST_ERROR;
            goto cleanup;
        }}
        copy->unit_len = unit_len;
        copy->flags = flags;
        copy->data = (const void *)(copy + 1);
        size_t unit_bytes = (size_t)unit_len * (flags == KILN_STRING_UTF16 ? sizeof(uint16_t) : sizeof(uint8_t));
        if (unit_bytes != 0 && src_str != NULL && src_str->data != NULL) {{
            memcpy((void *)copy->data, src_str->data, unit_bytes);
        }}
        owned->value.ptr = (const char *)copy;
        owned->value.flags = 0;
        owned->value.len = unit_len;
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
        "\ntypedef struct {wrapper_symbol}_gc_context {{\n    kiln_value **imports;\n    kiln_value **exports;\n    kiln_value *module_namespaces;\n    kiln_value *result;\n}} {wrapper_symbol}_gc_context;"
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
    let opentui_mark = if modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import())
    {
        "\n    (void)kiln_realm_gc_mark_value(heap, &kiln_shared_opentui_namespace);\n    (void)kiln_realm_gc_mark_value(heap, &kiln_shared_node_ffi_namespace);"
    } else {
        ""
    };
    writeln!(
        output,
        "static void {wrapper_symbol}_gc_roots(void *heap, void *context, int phase) {{\n    {wrapper_symbol}_gc_context *roots = ({wrapper_symbol}_gc_context *)context;\n    size_t module;\n    size_t index;\n    if (phase == KILN_GC_PHASE_RESET || roots == NULL) return;\n    for (module = 0; module < (size_t){count}; ++module) {{\n        if (roots->imports != NULL && roots->imports[module] != NULL)\n            for (index = 0; index < {wrapper_symbol}_gc_import_counts[module]; ++index)\n                (void)kiln_realm_gc_mark_value(heap, &roots->imports[module][index]);\n        if (roots->exports != NULL && roots->exports[module] != NULL)\n            for (index = 0; index < {wrapper_symbol}_gc_export_counts[module]; ++index)\n                (void)kiln_realm_gc_mark_value(heap, &roots->exports[module][index]);\n        if (roots->module_namespaces != NULL) (void)kiln_realm_gc_mark_value(heap, &roots->module_namespaces[module]);\n    }}\n    if (roots->result != NULL) (void)kiln_realm_gc_mark_value(heap, roots->result);{opentui_mark}\n}}",
        count = modules.len()
    )
    .unwrap();
}

#[cfg(test)]
fn graph_shared_wrapper_source(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    wrapper_symbol: &str,
) -> Result<String, NativeError> {
    graph_shared_wrapper_source_with_failures(
        graph,
        modules,
        wrapper_symbol,
        &BTreeMap::new(),
        &BTreeMap::new(),
    )
}

fn graph_shared_wrapper_source_with_failures(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    wrapper_symbol: &str,
    native_failures: &BTreeMap<(PathBuf, String), String>,
    native_runtime_failures: &BTreeMap<(PathBuf, PathBuf, String), String>,
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
    let namespace_plans = graph_namespace_plans(graph, modules, &indices, None, None)?;
    let entry_index = modules
        .iter()
        .position(|(node, _)| node.path == graph.entry())
        .ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::InternalMissingModule {
                path: graph.entry().to_path_buf(),
            })
        })?;
    let logical_paths = modules
        .iter()
        .map(|(node, _)| logical_module_path(graph.project_root(), &node.path))
        .collect::<Result<Vec<_>, _>>()?;
    let mut runtime_importers = BTreeMap::<PathBuf, usize>::new();
    for (node, _) in modules {
        for dependency in &node.runtime_dependencies {
            let next = runtime_importers.len();
            runtime_importers
                .entry(dependency.importer.clone())
                .or_insert(next);
        }
        for ((importer, _), _) in graph.runtime_failures(&node.path) {
            let next = runtime_importers.len();
            runtime_importers.entry(importer.clone()).or_insert(next);
        }
        for ((owner, importer, _), _) in native_runtime_failures {
            if owner == &node.path {
                let next = runtime_importers.len();
                runtime_importers.entry(importer.clone()).or_insert(next);
            }
        }
    }
    let mut output = String::with_capacity(embedding_header().len() + modules.len() * 768);
    output.push_str("/* Kiln native shared graph wrapper. */\n");
    output.push_str(
        "#include <stddef.h>\n#include <stdint.h>\n#include <stdlib.h>\n#include <string.h>\n#ifndef _WIN32\n#include <unistd.h>\n#endif\n\n",
    );
    output.push_str(embedding_header());
    output.push('\n');
    output.push_str(
        "static inline int kiln_c99_string_equals_ascii(kiln_value value, const char *ascii, size_t length) { if (value.tag != KILN_VALUE_STRING || value.ptr == NULL) return 0; return kiln_string_equals_ascii((const kiln_string *)value.ptr, ascii, length); }\n\
         static inline int kiln_c99_string_starts_ascii(kiln_value value, const char *ascii, size_t length) { if (value.tag != KILN_VALUE_STRING || value.ptr == NULL) return 0; return kiln_string_starts_ascii((const kiln_string *)value.ptr, ascii, length); }\n\
         static int kiln_c99_filename_matches(kiln_value value, const char *path, size_t path_len) { if (path == NULL || value.tag != KILN_VALUE_STRING || (value.len != 0 && value.ptr == NULL)) return 0; const kiln_string *s = (const kiln_string *)value.ptr; if (s == NULL) return 0; if (s->unit_len == path_len && (path_len == 0 || kiln_string_equals_ascii(s, path, path_len))) return 1; if (s->unit_len >= 7 && kiln_string_starts_ascii(s, \"file://\", 7) && s->unit_len - 7 == path_len) { if (path_len == 0) return 1; for (size_t i = 0; i < path_len; ++i) { if (kiln_string_unit_at(s, i + 7) != (uint8_t)path[i]) return 0; } return 1; } return 0; }\n",
    );
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
#define KILN_C99_HINT_STRING 1
#define KILN_C99_HOST_MODULE_CREATE_REQUIRE UINT16_C(0x7FF0)
extern int kiln_realm_is_active(void);

typedef int (*kiln_graph_module_fn)(kiln_host *host, kiln_value *result,
                                    const kiln_value *imports, size_t import_count,
                                    kiln_value *exports, size_t export_count);
typedef int (*kiln_commonjs_require_fn)(void *context, kiln_host *host, kiln_value specifier,
                                        kiln_value *result, kiln_value *thrown);
typedef struct kiln_commonjs_context kiln_commonjs_context;
extern int kiln_c99_commonjs_require_value(kiln_value *out, kiln_commonjs_require_fn call, void *context);
extern int kiln_c99_default_require(void *context, kiln_host *host, kiln_value specifier,
                                    kiln_value *result, kiln_value *thrown);
extern int kiln_c99_get(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_new_object(kiln_value *result);
extern int kiln_c99_put(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value value, int depth, kiln_value *thrown);
extern kiln_value kiln_c99_copy_text(const char *text, size_t len);
extern kiln_value kiln_c99_undefined(void);
extern int kiln_c99_to_primitive(kiln_host *host, kiln_value value, int hint, kiln_value *out, int depth, kiln_value *thrown);
extern kiln_value kiln_c99_text(kiln_value value);
extern int kiln_c99_throw_error(kiln_value *out, uint64_t kind, const char *message);
extern int kiln_c99_new_type_error(kiln_value *out, const char *message);
extern int kiln_c99_invoke_method(kiln_host *host, kiln_value *result, uint64_t code,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *thrown);
extern int kiln_c99_module_completion(kiln_host *, kiln_value, uint32_t, kiln_value *, int, kiln_value *);
extern int kiln_c99_finish_module_checkpoint(kiln_host *, kiln_value *);
#define KILN_C99_OK 0
#define KILN_C99_METHOD_PROMISE_RESOLVE 110
#define KILN_C99_METHOD_PROMISE_REJECT 111
#define KILN_C99_METHOD_PROMISE_WITH_RESOLVERS 411
static __thread kiln_value *kiln_shared_namespaces = NULL;
static __thread kiln_commonjs_context *kiln_shared_commonjs = NULL;
"#,
    );
    if modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import())
    {
        output.push_str("static __thread kiln_value kiln_shared_opentui_namespace;\n");
        output.push_str("static __thread kiln_value kiln_shared_node_ffi_namespace;\n");
    }
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

typedef struct kiln_commonjs_graph_context kiln_commonjs_graph_context;
typedef struct kiln_shared_require_context {
    kiln_commonjs_graph_context *graph;
    size_t base_index;
} kiln_shared_require_context;
struct kiln_commonjs_graph_context {
    kiln_commonjs_context *modules;
    kiln_value **exports;
    size_t module_count;
    kiln_shared_require_context *require_contexts;
};

static int kiln_graph_load_commonjs(kiln_commonjs_context *context, kiln_value *result,
                                    kiln_value *thrown);

static int kiln_shared_ensure_target(kiln_commonjs_graph_context *graph, size_t index,
                                     kiln_value *result, kiln_value *thrown) {
    int status;
    if (graph == NULL || result == NULL || index >= graph->module_count ||
        graph->modules == NULL || graph->exports == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (graph->modules[index].module != NULL) {
        status = kiln_graph_load_commonjs(&graph->modules[index], result, thrown);
        return status;
    }
    return KILN_C99_HOST_ERROR;
}

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
    /* The graph context is owned by the shared wrapper's realm frame.  Keeping this pointer
     * beside the forwarding host gives later graph callbacks a stable route back to the graph
     * without replacing the embedding host's call/context pair or using process-global state. */
    kiln_commonjs_graph_context *graph;
    kiln_proxy_arena *arena;
    int failure_status;
} kiln_proxy_host;

static int kiln_shared_create_require(kiln_proxy_host *proxy, uint16_t argc,
                                      const kiln_value *arguments, kiln_value *result);

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
    kiln_string *copy;
    kiln_value normalized_small[16];
    kiln_value *normalized_arguments = NULL;
    int status;
    if (proxy == NULL || proxy->upstream == NULL || proxy->upstream->call == NULL ||
        result == NULL) return KILN_C99_INVALID_ARGUMENT;
    upstream = proxy->upstream;
    arena = proxy->arena;
    if (operation == KILN_C99_HOST_MODULE_CREATE_REQUIRE)
        return kiln_shared_create_require(proxy, argc, arguments, result);
    if (argc != 0) {
        if (arguments == NULL || (size_t)argc > (size_t)-1 / sizeof(*normalized_arguments)) {
            proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
            return KILN_C99_INVALID_ARGUMENT;
        }
        normalized_arguments = argc <= 16 ? normalized_small :
            (kiln_value *)malloc((size_t)argc * sizeof(*normalized_arguments));
        if (normalized_arguments == NULL) {
            proxy->failure_status = KILN_C99_HOST_ERROR;
            return KILN_C99_HOST_ERROR;
        }
        memcpy(normalized_arguments, arguments, (size_t)argc * sizeof(*normalized_arguments));
        for (uint16_t index = 0; index < argc; ++index) normalized_arguments[index].flags = 0;
    }
    status = upstream->call(upstream->context, operation, argc, normalized_arguments, result);
    if (normalized_arguments != normalized_small) free(normalized_arguments);
    result->flags = 0;
    if (status != KILN_C99_OK) return status;
    if (result->tag != KILN_VALUE_STRING) return KILN_C99_OK;
    if (result->len == (uint32_t)-1 || (result->len != 0 && result->ptr == NULL)) {
        proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
        return KILN_C99_INVALID_ARGUMENT;
    }
    {
        const kiln_string *src_str = (const kiln_string *)result->ptr;
        size_t storage_size = 0;
        uint32_t flags = src_str != NULL ? src_str->flags : KILN_STRING_LATIN1;
        uint32_t unit_len = (uint32_t)result->len;
        if (!kiln_string_storage_size(unit_len, flags, &storage_size)) {
            proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
            return KILN_C99_INVALID_ARGUMENT;
        }
        copy = (kiln_string *)malloc(storage_size);
        if (copy == NULL) {
            proxy->failure_status = KILN_C99_HOST_ERROR;
            return KILN_C99_HOST_ERROR;
        }
        copy->unit_len = unit_len;
        copy->flags = flags;
        copy->data = (const void *)(copy + 1);
        size_t unit_bytes = (size_t)unit_len * (flags == KILN_STRING_UTF16 ? sizeof(uint16_t) : sizeof(uint8_t));
        if (unit_bytes != 0 && src_str != NULL && src_str->data != NULL) {
            memcpy((void *)copy->data, src_str->data, unit_bytes);
        }
        status = kiln_proxy_arena_track(arena, copy);
        if (status != KILN_C99_OK) {
            free(copy);
            proxy->failure_status = KILN_C99_HOST_ERROR;
            return KILN_C99_HOST_ERROR;
        }
        result->ptr = (const char *)copy;
        result->flags = 0;
        result->len = unit_len;
        return KILN_C99_OK;
    }
}

/* Shared graph callbacks receive the wrapper host.  These accessors are deliberately tiny and
 * private to the generated wrapper: the original embedding host remains the sole owner of host
 * operations, while the graph context lifetime follows the realm frame that registered roots. */
static kiln_commonjs_graph_context *kiln_graph_context_from_host(kiln_host *host) {
    kiln_proxy_host *proxy;
    if (host == NULL || host->call != kiln_proxy_call || host->context == NULL) return NULL;
    proxy = (kiln_proxy_host *)host->context;
    return proxy->graph;
}

"#,
    );
    for (index, (node, _)) in modules.iter().enumerate() {
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
                    "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) return kiln_graph_load_commonjs(&graph->modules[{provider_index}], result, thrown);"
                )
                .unwrap();
            } else {
                let Some(commonjs_export) =
                    native_commonjs_projection_index(modules[provider_index].0)
                else {
                    continue;
                };
                writeln!(
                    output,
                    "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ *result = graph->exports[{provider_index}][{commonjs_export}]; return KILN_C99_OK; }}"
                )
                .unwrap();
            }
        }
        output.push_str(
            "    if (thrown != NULL) *thrown = specifier;\n    return KILN_C99_HOST_ERROR;\n}\n",
        );
    }
    output.push_str("\nstatic int kiln_shared_require_builtin(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {\n    kiln_commonjs_graph_context *graph = (kiln_commonjs_graph_context *)opaque;\n    (void)host;\n    if (graph == NULL || result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n");
    for (index, (node, _)) in modules.iter().enumerate() {
        let ModuleSource::NodeBuiltin(builtin) = node.source else {
            continue;
        };
        let Some(projection) = native_commonjs_projection_index(node) else {
            continue;
        };
        let specifier = builtin.specifier();
        writeln!(output, "    if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ *result = graph->exports[{}][{}]; return KILN_C99_OK; }}", native_c_string(specifier), specifier.len(), index, projection).unwrap();
    }
    output.push_str("    return kiln_c99_default_require(NULL, host, specifier, result, thrown);\n}\n\nstatic int kiln_shared_runtime_require(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {\n    kiln_shared_require_context *context = (kiln_shared_require_context *)opaque;\n    kiln_commonjs_graph_context *graph;\n    if (context == NULL || (graph = context->graph) == NULL || result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n    (void)host;\n    switch (context->base_index) {\n");
    for (base_index, importer) in runtime_importers.keys().enumerate() {
        writeln!(output, "        case {base_index}:").unwrap();
        for node in modules.iter().map(|(node, _)| node) {
            for dependency in &node.runtime_dependencies {
                if dependency.importer != *importer {
                    continue;
                }
                let Some(&provider_index) = indices.get(&dependency.target) else {
                    continue;
                };
                let specifier = native_c_string(&dependency.specifier);
                let specifier_len = dependency.specifier.len();
                if modules[provider_index].0.source == ModuleSource::CommonJs {
                    writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) return kiln_shared_ensure_target(graph, (size_t){provider_index}, result, thrown);").unwrap();
                } else if let Some(projection) =
                    native_commonjs_projection_index(modules[provider_index].0)
                {
                    writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ *result = graph->exports[{provider_index}][{projection}]; return KILN_C99_OK; }}").unwrap();
                }
            }
        }
        for node in modules.iter().map(|(node, _)| node) {
            for ((failure_importer, failure_specifier), message) in
                graph.runtime_failures(&node.path)
            {
                if failure_importer != importer {
                    continue;
                }
                writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ kiln_value error = kiln_c99_undefined(); (void)kiln_c99_new_type_error(&error, {}); if (thrown != NULL) *thrown = error; return KILN_C99_THROWN; }}", native_c_string(failure_specifier), failure_specifier.len(), native_c_string(message)).unwrap();
            }
        }
        for ((owner, failure_importer, failure_specifier), message) in native_runtime_failures {
            if failure_importer != importer {
                continue;
            }
            if !modules.iter().any(|(node, _)| node.path == *owner) {
                continue;
            }
            writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ kiln_value error = kiln_c99_undefined(); (void)kiln_c99_new_type_error(&error, {}); if (thrown != NULL) *thrown = error; return KILN_C99_THROWN; }}", native_c_string(failure_specifier), failure_specifier.len(), native_c_string(message)).unwrap();
        }
        output.push_str("            break;\n");
    }
    output.push_str("        default: break;\n    }\n    return kiln_c99_default_require(NULL, host, specifier, result, thrown);\n}\n\nstatic int kiln_shared_create_require(kiln_proxy_host *proxy, uint16_t argc, const kiln_value *arguments, kiln_value *result) {\n    kiln_commonjs_graph_context *graph;\n    if (proxy == NULL || result == NULL || argc != 1 || arguments == NULL || arguments[0].tag != KILN_VALUE_STRING || (arguments[0].len != 0 && arguments[0].ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n    graph = proxy->graph;\n    if (graph == NULL || graph->require_contexts == NULL) return KILN_C99_HOST_ERROR;\n");
    for (base_index, importer) in runtime_importers.keys().enumerate() {
        let importer = importer.to_string_lossy();
        let file_url = Url::from_file_path(importer.as_ref())
            .ok()
            .map(|url| url.to_string());
        let logical_alias = modules
            .iter()
            .find(|(node, _)| node.path == *importer)
            .map(|_| logical_module_path(graph.project_root(), Path::new(importer.as_ref())))
            .transpose()?;
        let aliases = format!(
            "{}{}",
            file_url
                .as_ref()
                .map(|url| format!(
                    " || kiln_c99_filename_matches(arguments[0], {}, (size_t){})",
                    native_c_string(url),
                    url.len()
                ))
                .unwrap_or_default(),
            logical_alias
                .as_ref()
                .map(|path| format!(
                    " || kiln_c99_filename_matches(arguments[0], {}, (size_t){})",
                    native_c_string(path.as_str()),
                    path.as_str().len()
                ))
                .unwrap_or_default()
        );
        writeln!(output, "    if (kiln_c99_filename_matches(arguments[0], {}, (size_t){}){} ) {{ graph->require_contexts[{base_index}].graph = graph; graph->require_contexts[{base_index}].base_index = (size_t){base_index}; return kiln_c99_commonjs_require_value(result, kiln_shared_runtime_require, &graph->require_contexts[{base_index}]); }}", native_c_string(importer.as_ref()), importer.len(), aliases).unwrap();
    }
    output.push_str("    return kiln_c99_commonjs_require_value(result, kiln_shared_require_builtin, graph);\n}\n");
    let has_any_dynamic_import = modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import());
    if has_any_dynamic_import {
        write_shared_wrapper_dynamic_import(
            &mut output,
            graph,
            modules,
            &logical_paths,
            native_failures,
        );
    }
    render_shared_wrapper_roots(&mut output, modules, wrapper_symbol);
    let opentui_init = if has_any_dynamic_import {
        "\n    kiln_shared_opentui_namespace = (kiln_value){ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };\n    kiln_shared_node_ffi_namespace = (kiln_value){ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };"
    } else {
        ""
    };
    writeln!(
        output,
        r#"
typedef struct {wrapper_symbol}_realm_context {{
    kiln_value *result;
    kiln_proxy_arena arena;
    kiln_proxy_host proxy;
    kiln_host proxy_host;
    kiln_value **shared_import_arrays;
    kiln_value **shared_export_arrays;
    kiln_value *shared_namespaces;
    kiln_commonjs_context *shared_commonjs_modules;
    kiln_commonjs_graph_context shared_commonjs_graph;
    kiln_shared_require_context require_contexts[{runtime_context_count}];
    {wrapper_symbol}_gc_context shared_gc_context;
    kiln_value shared_module_result;
    int roots_registered;
}} {wrapper_symbol}_realm_context;
"#,
        wrapper_symbol = wrapper_symbol,
        runtime_context_count = runtime_importers.len().max(1),
    )
    .unwrap();
    writeln!(
        output,
        "\nstatic int {wrapper_symbol}_body({wrapper_symbol}_realm_context *context) {{\n    kiln_host *proxy_host;\n    kiln_value *result;\n    kiln_value **import_arrays = NULL;\n    kiln_value **export_arrays = NULL;\n    kiln_value *namespaces = NULL;\n    kiln_commonjs_context *commonjs_modules = NULL;\n    kiln_commonjs_graph_context *commonjs_graph;\n    {wrapper_symbol}_gc_context *gc_context;\n    /* All graph storage belongs to the realm context and survives this body until drive and\n     * realm teardown have completed. */\n    #define module_result (context->shared_module_result)\n    int status = KILN_C99_OK;\n    size_t index;\n    if (context == NULL) return KILN_C99_INVALID_ARGUMENT;\n    proxy_host = &context->proxy_host;\n    result = context->result;\n    import_arrays = context->shared_import_arrays;\n    export_arrays = context->shared_export_arrays;\n    namespaces = context->shared_namespaces;\n    commonjs_modules = context->shared_commonjs_modules;\n    commonjs_graph = &context->shared_commonjs_graph;\n    gc_context = &context->shared_gc_context;\n    if (result == NULL || proxy_host == NULL) return KILN_C99_INVALID_ARGUMENT;\n    *commonjs_graph = (kiln_commonjs_graph_context){{ NULL, NULL, (size_t){count} }};\n    *result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};\n    if ((size_t){count} > (size_t)-1 / sizeof(*import_arrays) ||\n        (size_t){count} > (size_t)-1 / sizeof(*export_arrays) ||\n        (size_t){count} > (size_t)-1 / sizeof(*namespaces) ||\n        (size_t){count} > (size_t)-1 / sizeof(*commonjs_modules)) {{\n        status = KILN_C99_HOST_ERROR;\n        goto cleanup;\n    }}\n    import_arrays = (kiln_value **)calloc((size_t){count}, sizeof(*import_arrays));\n    context->shared_import_arrays = import_arrays;\n    export_arrays = (kiln_value **)calloc((size_t){count}, sizeof(*export_arrays));\n    context->shared_export_arrays = export_arrays;\n    namespaces = (kiln_value *)calloc((size_t){count}, sizeof(*namespaces));\n    context->shared_namespaces = namespaces;\n    commonjs_modules = (kiln_commonjs_context *)calloc((size_t){count}, sizeof(*commonjs_modules));\n    context->shared_commonjs_modules = commonjs_modules;\n    if (import_arrays == NULL || export_arrays == NULL || namespaces == NULL || commonjs_modules == NULL) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}\n    commonjs_graph->modules = commonjs_modules;\n    commonjs_graph->exports = export_arrays;\n    /* The wrapper host carries the graph context for callbacks, but preserves the embedding\n     * host's function and context verbatim for every host operation. */\n    if (proxy_host->context != NULL && proxy_host->call == kiln_proxy_call)\n        ((kiln_proxy_host *)proxy_host->context)->graph = commonjs_graph;\n    if (kiln_graph_context_from_host(proxy_host) != commonjs_graph) {{ status = KILN_C99_INVALID_ARGUMENT; goto cleanup; }}\n    gc_context->imports = import_arrays;\n    gc_context->exports = export_arrays;\n    gc_context->module_namespaces = namespaces;\n    gc_context->result = &module_result;\n    module_result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};\n    status = kiln_realm_gc_register_roots({wrapper_symbol}_gc_roots, gc_context, KILN_GC_ROOT_SURVIVES_TEARDOWN);\n    if (status != KILN_C99_OK) goto cleanup;\n    context->roots_registered = 1;\n    kiln_shared_namespaces = namespaces;\n    kiln_shared_commonjs = commonjs_modules;{opentui_init}",
        wrapper_symbol = wrapper_symbol,
        count = modules.len(),
    )
    .unwrap();
    output.push_str("    commonjs_graph->require_contexts = context->require_contexts;\n");
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
                    if modules[provider_index].0.source == ModuleSource::CommonJs {
                        writeln!(
                            output,
                            "    status = kiln_graph_load_commonjs(&commonjs_modules[{provider_index}], &module_result, &module_result);\n    if (status != KILN_C99_OK) {{ *result = module_result; goto cleanup; }}\n    namespaces[{provider_index}] = export_arrays[{provider_index}][0];"
                        )
                        .unwrap();
                    }
                    writeln!(
                        output,
                        "    import_arrays[{index}][{import_index}] = namespaces[{provider_index}];"
                    )
                    .unwrap();
                }
                NativeGraphImport::CommonJsNamed {
                    provider_index,
                    property,
                } => {
                    writeln!(
                        output,
                        "    status = kiln_graph_load_commonjs(&commonjs_modules[{provider_index}], &module_result, &module_result);\n    if (status != KILN_C99_OK) {{ *result = module_result; goto cleanup; }}"
                    )
                    .unwrap();
                    let prop_c = native_c_string(&property);
                    let prop_len = property.len();
                    writeln!(
                        output,
                        "    status = kiln_c99_get(proxy_host, export_arrays[{provider_index}][0], {prop_c}, (size_t){prop_len}, &import_arrays[{index}][{import_index}], 0, &module_result);\n    if (status != KILN_C99_OK) {{ *result = module_result; goto cleanup; }}"
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
            writeln!(
                output,
                "    namespaces[{index}] = export_arrays[{index}][0];"
            )
            .unwrap();
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
            "    module_result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};\n    status = {}(proxy_host, &module_result, import_arrays[{index}], (size_t){import_count}, export_arrays[{index}], (size_t){export_count});\n    if (status != KILN_C99_OK) {{ *result = module_result; goto cleanup; }}",
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
    let opentui_cleanup = if has_any_dynamic_import {
        "\n    kiln_shared_opentui_namespace = (kiln_value){ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };\n    kiln_shared_node_ffi_namespace = (kiln_value){ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };"
    } else {
        ""
    };
    output.push_str("cleanup:\n    #undef module_result\n    return status;\n}\n");
    writeln!(
        output,
        r#"
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
    return {wrapper_symbol}_body(context);
}}

static int {impl_symbol}(kiln_host *host, kiln_value *result, int preserve_result) {{
    {wrapper_symbol}_realm_context context;
    uint64_t realm = 0;
    kiln_string *preserved = NULL;
    int status;
    int destroy_status;
    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;
    memset(&context, 0, sizeof(context));
    context.result = result;
    context.arena = (kiln_proxy_arena){{ NULL, 0, 0 }};
    context.proxy = (kiln_proxy_host){{ host, NULL, &context.arena, KILN_C99_OK }};
    context.proxy_host = (kiln_host){{ kiln_proxy_call, &context.proxy }};
    status = kiln_realm_create(&realm);
    if (status == KILN_C99_OK)
        status = kiln_realm_enter(realm, {wrapper_symbol}_realm_entry, &context);
    if (status == KILN_C99_OK)
        status = kiln_realm_drive(realm, &context.proxy_host, result);
    /* Public v1 results remain borrowed; private v2 calls preserve strings across cleanup. */
    if (preserve_result != 0 && result->tag == KILN_VALUE_STRING) {{
        if (result->len == (uint32_t)-1 || (result->len != 0 && result->ptr == NULL)) {{
            *result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};
            status = KILN_C99_INVALID_ARGUMENT;
        }} else {{
            const kiln_string *src_str = (const kiln_string *)result->ptr;
            size_t storage_size = 0;
            uint32_t flags = src_str != NULL ? src_str->flags : KILN_STRING_LATIN1;
            uint32_t unit_len = (uint32_t)result->len;
            if (!kiln_string_storage_size(unit_len, flags, &storage_size)) {{
                *result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};
                status = KILN_C99_INVALID_ARGUMENT;
            }} else {{
                preserved = (kiln_string *)malloc(storage_size);
                if (preserved == NULL) {{
                    *result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};
                    status = KILN_C99_HOST_ERROR;
                }} else {{
                    preserved->unit_len = unit_len;
                    preserved->flags = flags;
                    preserved->data = (const void *)(preserved + 1);
                    size_t unit_bytes = (size_t)unit_len * (flags == KILN_STRING_UTF16 ? sizeof(uint16_t) : sizeof(uint8_t));
                    if (unit_bytes != 0 && src_str != NULL && src_str->data != NULL) {{
                        memcpy((void *)preserved->data, src_str->data, unit_bytes);
                    }}
                }}
            }}
        }}
    }}
    if (realm != 0) {{
        destroy_status = kiln_realm_destroy(realm);
        if (status == KILN_C99_OK) status = destroy_status;
    }}
    kiln_shared_namespaces = NULL;
    kiln_shared_commonjs = NULL;{opentui_cleanup}
    for (size_t cleanup_index = 0; cleanup_index < (size_t){count}; ++cleanup_index) {{
        free(context.shared_import_arrays == NULL ? NULL : context.shared_import_arrays[cleanup_index]);
        free(context.shared_export_arrays == NULL ? NULL : context.shared_export_arrays[cleanup_index]);
    }}
    free(context.shared_import_arrays);
    free(context.shared_export_arrays);
    free(context.shared_namespaces);
    free(context.shared_commonjs_modules);
    kiln_proxy_arena_clear(&context.arena);
    if (preserved != NULL) result->ptr = (const char *)preserved;
    result->flags = 0;
    return status;
}}

int {wrapper_symbol}(kiln_host *host, kiln_value *result) {{
    return {impl_symbol}(host, result, 0);
}}
"#,
        count = modules.len(),
    )
    .unwrap();
    let invocation = format!("{impl_symbol}(&proxy_host, &value, 1)");
    output.push_str(&owned_result_wrapper_source(
        wrapper_symbol,
        "graph",
        &invocation,
        "if (value.tag == KILN_VALUE_STRING) { free((void *)value.ptr); value.ptr = NULL; value.len = 0; }",
    ));
    Ok(output.replace(
        "UINT16_C(0x7FF0)",
        &format!("UINT16_C(0x{:04X})", C99_HOST_MODULE_CREATE_REQUIRE),
    ))
}

/// Parse the `KILN_WORKERS` environment setting, matching the launcher's `kiln_cli_worker_count` C logic.
///
/// Semantics:
/// - Unset or empty: 1
/// - `"auto"` (case-insensitive): number of online CPU cores via `sysconf(_SC_NPROCESSORS_ONLN)` on Unix/Linux (1 on Windows), clamped to `1..=1024`
/// - Valid integer string >= 1: clamped to `1..=1024`
/// - Invalid integer string or integer < 1: 1
pub fn parse_worker_count(setting: Option<&str>) -> usize {
    parse_worker_count_with(setting, default_online_cpus)
}

/// Parse the `KILN_WORKERS` setting using a custom provider for online CPU cores.
pub fn parse_worker_count_with<F>(setting: Option<&str>, online_cpus: F) -> usize
where
    F: FnOnce() -> usize,
{
    let Some(setting) = setting else { return 1 };
    if setting.is_empty() {
        return 1;
    }
    if setting.eq_ignore_ascii_case("auto") {
        let count = online_cpus();
        count.clamp(1, 1024)
    } else {
        match setting.parse::<isize>() {
            Ok(count) if count >= 1 => (count as usize).min(1024),
            _ => 1,
        }
    }
}

fn default_online_cpus() -> usize {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn sysconf(name: std::ffi::c_int) -> std::ffi::c_long;
        }
        #[cfg(target_os = "linux")]
        const _SC_NPROCESSORS_ONLN: std::ffi::c_int = 84;
        #[cfg(target_os = "macos")]
        const _SC_NPROCESSORS_ONLN: std::ffi::c_int = 58;
        #[cfg(target_os = "freebsd")]
        const _SC_NPROCESSORS_ONLN: std::ffi::c_int = 58;
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))]
        let cpus = -1;
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
        let cpus = unsafe { sysconf(_SC_NPROCESSORS_ONLN) };

        if cpus > 0 {
            cpus as usize
        } else {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        }
    }
    #[cfg(not(unix))]
    {
        1
    }
}

/// Parse the HTTP/3 enablement setting matching the launcher's `kiln_cli_http3_enabled` C logic.
///
/// Semantics:
/// - Enabled if `KILN_HTTP3` is set and not empty and not `"0"`.
/// - Enabled if command-line arguments include `"--http3"`.
/// - Otherwise disabled.
pub fn parse_http3_enabled<I, S>(env_setting: Option<&str>, args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if let Some(setting) = env_setting
        && !setting.is_empty()
        && setting != "0"
    {
        return true;
    }
    args.into_iter().any(|arg| arg.as_ref() == "--http3")
}

#[cfg(test)]
mod tests;
