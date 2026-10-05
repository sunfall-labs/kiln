//! Cached orchestration for Kiln's C native backend.
//!
//! The backend, runtime, cache, and platform toolchain deliberately live in separate crates.
//! This crate is the narrow coordinator that gives those pieces an ordered build graph:
//! generated module C, application object, reusable runtime object, launcher object, executable.

mod number_provider;
mod provider_link_input;
pub mod shared_publication;

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::hash::{Hash, Hasher};
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
use kiln_ir::Program;
use kiln_module_graph::{ModuleGraph, ModuleGraphError, ModuleSource, ResolvedExport};
use kiln_module_resolver::NodeBuiltin;
pub use kiln_native_provider_contract::{
    ProviderCapabilities, ProviderCapability, ProviderVariant,
};
pub use kiln_native_runtime::{ChildStream, embedding_header, embedding_module_header};
use kiln_native_runtime::{
    EMBEDDING_C_ABI_VERSION, EMBEDDING_HOST_ABI_VERSION, EMBEDDING_MODULE_API_VERSION,
    NATIVE_RESULT_PATH_ENV, NATIVE_RUNTIME_VERSION, decode_native_result,
    embedding_module_api_symbol, launcher_source, runtime_source,
};
use kiln_native_toolchain::{
    CompileOptions, CompilerKind, LinkOptions, ToolIdentity, Toolchain, ToolchainError,
    native_release_keep_symbols,
};
use number_provider::{NumberProvider, PROVIDER_ABI_VERSION, PROVIDER_SCHEMA_VERSION};
use rayon::prelude::*;
use url::Url;

static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);
static MATERIALIZE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A release build (`release_bootstrap_specialized`) emits the builtin bootstrap
/// specialized and compiles it at -O2 (issue 17), in translation units of about this many bytes
/// of generated C, so its one-time compile is parallel units instead of one minute-long one. The
/// bootstrap is not subject to the user-module specialization budget: it is one module per
/// toolchain and build configuration, cached by content, and its methods are hot loops.
const RELEASE_BOOTSTRAP_UNIT_BYTES: usize = 1 << 20;

/// Whether a release build emits the bootstrap specialized: yes, unless
/// `KILN_BOOTSTRAP_SPECIALIZED=0`. It was opt-in until the specialized emitter's miscompile of
/// `headerListRemove` (`Headers.set`/`delete` never returned: a versioned body's generic clone
/// read a speculated value's twin it never wrote) was fixed -- parallel-builtins issue 18 -- and
/// a `native-release` pass over every `conformance/native` probe answered the same with the
/// bootstrap specialized as generic. The opt-out is for bisecting a release-only divergence: it
/// restores the generic `-O0` bootstrap.
fn release_bootstrap_specialized() -> bool {
    std::env::var("KILN_BOOTSTRAP_SPECIALIZED").as_deref() != Ok("0")
}

// Object misses for the same or colliding bucket share one process mutex and one OS lock. The
// cache key is rechecked after both locks are held, so bucket collisions only serialize work and
// never authorize reuse of another key's artifact. Lock files are kept outside artifact-kind
// trees, where cache GC scans records, and are bounded to at most 256 per writable layer.
const OBJECT_COMPILE_LOCK_BUCKETS: usize = 256;
static OBJECT_COMPILE_LOCKS: [parking_lot::Mutex<()>; OBJECT_COMPILE_LOCK_BUCKETS] =
    [const { parking_lot::Mutex::new(()) }; OBJECT_COMPILE_LOCK_BUCKETS];

fn needs_native_http_facade(members: &kiln_graph::DemandedMembers) -> bool {
    if members
        .globals
        .iter()
        .any(|name| matches!(name.as_str(), "fetch" | "Bun" | "Kiln"))
        || members.has_open_world
    {
        return true;
    }

    let uses_global_object = members
        .globals
        .iter()
        .any(|name| matches!(name.as_str(), "globalThis" | "global"));
    if !uses_global_object {
        return false;
    }

    members
        .names
        .iter()
        .any(|name| matches!(name.as_str(), "fetch" | "Bun" | "Kiln"))
        || members.has_computed_access
        || members
            .globals
            .iter()
            .any(|name| matches!(name.as_str(), "Reflect" | "Object"))
        || members
            .names
            .iter()
            .any(|name| matches!(name.as_str(), "get" | "getOwnPropertyDescriptor" | "has"))
}

const RELEASE_PRELUDE_COMPILE_IDENTITY: &[u8] = b"vectorize=off\0slp-vectorize=off";

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

fn demanded_export_names(
    exports: &[kiln_ir::ExportBinding],
    slots: &BTreeSet<u32>,
) -> BTreeSet<String> {
    exports
        .iter()
        .filter(|export| slots.contains(&export.local_slot))
        .map(|export| export.exported.clone())
        .collect()
}

fn export_slots_for_names(
    exports: &[kiln_ir::ExportBinding],
    names: &BTreeSet<String>,
) -> BTreeSet<u32> {
    exports
        .iter()
        .filter(|export| names.contains(&export.exported))
        .map(|export| export.local_slot)
        .collect()
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
        let demanded_names = demanded_export_names(&node.program.exports, &demanded_slots);
        let Some(path) = node.path.to_str() else {
            continue;
        };
        let is_entry = node.path == graph.entry();
        let hir = match &node.program.program.static_source_provenance {
            Some(provenance) => kiln_frontend_oxc::parse_dce_native_prepared(
                path,
                &node.program.program.source,
                &demanded_names,
                is_entry,
                Arc::clone(provenance),
            ),
            None => kiln_frontend_oxc::parse_dce_native(
                path,
                &node.program.program.source,
                &demanded_names,
                is_entry,
            ),
        };
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

        // Source-level DCE can renumber the compiler-generated locals used to represent
        // indirect exports. Reconcile those links by public export name, just as the later
        // import.meta reparse does, rather than relying on a generated local name surviving
        // the reparse. Otherwise a barrel can retain the export row while losing the import
        // that supplies its value, which leaves the slot uninitialized in release builds.
        let _ = restore_reexport_imports(&node.program, &mut program);

        let replacement_slots = export_slots_for_names(&program.exports, &demanded_names);
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

fn restore_reexport_imports(
    previous: &kiln_ir::ModuleProgram,
    reparsed: &mut kiln_ir::ModuleProgram,
) -> BTreeSet<(usize, usize)> {
    let mut mapped_imports = BTreeSet::new();
    for export in &reparsed.exports {
        let Some(previous_export) = previous
            .exports
            .iter()
            .find(|previous| previous.exported == export.exported)
        else {
            continue;
        };
        let Some((previous_index, previous_import)) = previous
            .imports
            .iter()
            .enumerate()
            .find(|(_, import)| import.local_slot == previous_export.local_slot)
        else {
            continue;
        };

        let new_index = if let Some(index) = reparsed
            .imports
            .iter()
            .position(|import| import.local_slot == export.local_slot)
        {
            let import = &mut reparsed.imports[index];
            import.source.clone_from(&previous_import.source);
            import.imported.clone_from(&previous_import.imported);
            index
        } else {
            reparsed.imports.push(kiln_ir::ImportBinding {
                source: previous_import.source.clone(),
                imported: previous_import.imported.clone(),
                local: format!(
                    "{}reparsed-reexport-{}*",
                    kiln_ir::REEXPORT_LOCAL_PREFIX,
                    reparsed.imports.len()
                ),
                local_slot: export.local_slot,
            });
            reparsed.imports.len() - 1
        };
        mapped_imports.insert((previous_index, new_index));
    }
    mapped_imports
}

/// Re-parse modules with native `import.meta` (Node SEA semantics), preserving their
/// public export surface and remapping demanded local slots by export name. Recompilation
/// can remove unused locals and renumber the surviving export bindings, so carrying the
/// original slot numbers into emission is unsound even when every export remains present.
fn native_import_meta_reparse(
    graph: &ModuleGraph,
    demand: &kiln_module_graph::LiveDemand,
) -> (ModuleGraph, kiln_module_graph::LiveDemand) {
    let mut replacements = BTreeMap::new();
    let mut remapped = demand.clone();
    let mut remapped_imports = demand.live_imports.clone();

    for node in graph.modules() {
        if node.source != ModuleSource::Source {
            continue;
        }
        if node.program.program.source.is_empty() {
            continue;
        }
        // Demand all exports so reparsing can specialize dead code without changing the public
        // interface. The pass may remove unused imports and declarations as well.
        let demanded_names = node
            .program
            .exports
            .iter()
            .map(|export| export.exported.clone())
            .collect::<BTreeSet<_>>();
        let Some(path) = node.path.to_str() else {
            continue;
        };
        let is_entry = node.path == graph.entry();
        let hir = match &node.program.program.static_source_provenance {
            Some(provenance) => kiln_frontend_oxc::parse_dce_native_prepared(
                path,
                &node.program.program.source,
                &demanded_names,
                is_entry,
                Arc::clone(provenance),
            ),
            None => kiln_frontend_oxc::parse_dce_native(
                path,
                &node.program.program.source,
                &demanded_names,
                is_entry,
            ),
        };
        let Ok(mut program) = kiln_ir::compile_module(&hir) else {
            continue;
        };

        let reparsed_names = program
            .exports
            .iter()
            .map(|export| export.exported.clone())
            .collect::<BTreeSet<_>>();
        if !demanded_names.is_subset(&reparsed_names) {
            continue;
        }

        let mut imports = Vec::new();
        remapped_imports.retain(|(path, _)| path != &node.path);
        for mut import in program.imports {
            let Some((old_index, linked)) = node
                .program
                .imports
                .iter()
                .enumerate()
                .find(|(_, candidate)| candidate.local == import.local)
            else {
                continue;
            };
            import.source.clone_from(&linked.source);
            import.imported.clone_from(&linked.imported);
            let new_index = imports.len();
            if demand
                .live_imports
                .contains(&(node.path.clone(), old_index))
            {
                remapped_imports.insert((node.path.clone(), new_index));
            }
            imports.push(import);
        }
        program.imports = imports;

        // DCE may lower an imported local export as a fresh local binding, while the previous
        // graph represented it as an indirect export backed by a synthetic import slot. Preserve
        // that link relation by public export name; matching only the generated local name is
        // insufficient because the synthetic `*reexportN*` names are renumbered when exports are
        // removed. Without this edge, relinking silently turns the export into an uninitialized
        // local in the barrel and imports resolve to that slot instead of the original provider.
        let reexport_imports = restore_reexport_imports(&node.program, &mut program);
        for (previous_index, new_index) in reexport_imports {
            if demand
                .live_imports
                .contains(&(node.path.clone(), previous_index))
            {
                remapped_imports.insert((node.path.clone(), new_index));
            }
        }
        program.dependencies.clone_from(&node.program.dependencies);
        let demanded_names = demand
            .slots
            .get(&node.path)
            .map(|slots| demanded_export_names(&node.program.exports, slots))
            .unwrap_or_default();
        remapped.slots.insert(
            node.path.clone(),
            export_slots_for_names(&program.exports, &demanded_names),
        );
        replacements.insert(node.path.clone(), program);
    }

    remapped.live_imports = remapped_imports;

    match graph.replace_programs_and_relink(replacements) {
        Ok(graph) => (graph, remapped),
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

/// Opt-in release-graph export-slot trace. Set `KILN_DEBUG_MODULE_DEMANDS` to a module path
/// substring (for example, `acorn-typescript`) to compare export slots and demand at each graph
/// transformation boundary. This is out-of-band and does not affect cache keys or emission.
fn debug_module_demands(stage: &str, graph: &ModuleGraph, demand: &kiln_module_graph::LiveDemand) {
    let Ok(filter) = std::env::var("KILN_DEBUG_MODULE_DEMANDS") else {
        return;
    };
    if filter.is_empty() {
        return;
    }
    for node in graph
        .modules()
        .filter(|node| node.path.to_string_lossy().contains(filter.as_str()))
    {
        let exports = node
            .program
            .exports
            .iter()
            .map(|export| format!("{}={}", export.exported, export.local_slot))
            .collect::<Vec<_>>()
            .join(",");
        eprintln!(
            "kiln: module-demand stage={stage} path={} live={} demanded={:?} exports=[{}]",
            node.path.display(),
            demand.live.contains(&node.path),
            demand.slots.get(&node.path),
            exports
        );
        for (index, import) in node.program.imports.iter().enumerate() {
            let resolved = node
                .dependencies
                .iter()
                .find(|dependency| dependency.specifier == import.source)
                .and_then(|dependency| {
                    graph
                        .resolved_export(&dependency.target, &import.imported)
                        .map(|export| format!("{:?}", export))
                })
                .unwrap_or_else(|| "<unresolved-or-namespace>".to_owned());
            eprintln!(
                "kiln: module-demand-import stage={stage} path={} index={index} local={} local_slot={} source={} imported={} resolved={} live={}",
                node.path.display(),
                import.local,
                import.local_slot,
                import.source,
                import.imported,
                resolved,
                demand.live_imports.contains(&(node.path.clone(), index)),
            );
        }
    }
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

/// Every `parallelMap` site of a build and its decision, per module (its project-relative path;
/// `<main>` for a single-program build), as `NativeBuildReport::parallel_sites` holds them.
pub type ParallelSites = Vec<(String, Vec<kiln_backend_c::parallel::SiteRecord>)>;

/// How far a build goes: to the executable, or to the end of emission, which is as far as
/// anything needs to go to know what each `parallelMap` site decided.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BuildUntil {
    Executable,
    Emitted,
}

/// What a build that went [`BuildUntil`] somewhere produced. Made once per build and returned
/// straight away, so the size of the larger variant costs nothing worth a box.
#[allow(clippy::large_enum_variant)]
enum Built {
    Executable(NativeBuildReport),
    Emitted(ParallelSites),
}

/// The `parallelMap` site records the emitter wrote into one generated module (all of its
/// chunks), for `NativeBuildReport::parallel_sites`.
fn parallel_sites_of(
    module: &kiln_backend_c::CModule,
) -> Vec<kiln_backend_c::parallel::SiteRecord> {
    let mut sites = kiln_backend_c::parallel::SiteRecord::parse_all(&module.source);
    for chunk in &module.chunks {
        for site in kiln_backend_c::parallel::SiteRecord::parse_all(chunk) {
            if !sites.contains(&site) {
                sites.push(site);
            }
        }
    }
    sites
}

// Select the budget only on a generated-C cache miss. Warm builds must not rerun liveness,
// and the cache payload must remember the actual policy used to compile the emitted body.
fn emit_graph_module_for_profile(
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &EmissionPolicy,
    profile: NativeProfile,
    instantiate: bool,
) -> Result<kiln_backend_c::CModule, NativeError> {
    let policy = graph_source_emitter::effective_policy(node, policy, profile);
    if matches!(node.source, ModuleSource::Source | ModuleSource::CommonJs) {
        graph_source_emitter::emit(node, logical_path, &policy, instantiate)
    } else {
        graph::emit_native_graph_module_with_policy_and_instantiation(
            node,
            logical_path,
            &policy,
            instantiate,
        )
    }
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

/// The compile flags an object's cache key must name beyond the profile: `-march=native`, and the
/// inline-cache oracle `KILN_IC_CHECK` compiles in (`kiln-native-toolchain`). Without the second
/// an oracle build and an ordinary one shared object keys, so a `KILN_IC_CHECK=1` corpus run could
/// link objects compiled without the oracle -- and pass having checked nothing -- or an ordinary
/// build could link the oracle's slower objects.
fn object_flag_identity(options: &NativeBuildOptions) -> String {
    let mut identity = String::new();
    if options.march_native {
        identity.push_str("\0march-native");
    }
    if std::env::var_os("KILN_IC_CHECK").is_some_and(|v| !v.is_empty() && v != "0") {
        identity.push_str("\0ic-check");
    }
    identity
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
    executable_link_options_digest_with_symbols(profile, options, native_release_keep_symbols())
}

fn executable_link_options_digest_with_symbols(
    profile: NativeProfile,
    options: &NativeBuildOptions,
    keep_symbols: bool,
) -> Digest {
    let static_opt = if options.static_link { "\0static" } else { "" };
    let pgo_opt = pgo_options_fingerprint(options);
    let symbols_opt =
        if keep_symbols && matches!(profile, NativeProfile::Release | NativeProfile::ReleaseSize) {
            "\0keep-symbols"
        } else {
            ""
        };
    Digest::hash(
        format!(
            "{}\0system-math\0system-entropy-v1{static_opt}{pgo_opt}{symbols_opt}",
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
    use kiln_graph::{CORE_METHOD_GROUPS, PRUNABLE_METHOD_GROUPS};
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
    use kiln_graph::method_groups as groups;
    const FLAGS: [(u32, &str); 19] = [
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
        (groups::NODE_CRYPTO, "KILN_RETAIN_GROUP_NODE_CRYPTO"),
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
    assert!(PROVIDER_SCHEMA_VERSION == 5);
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
pub(crate) use provider_link_input::materialize_provider_archive;
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
    ) -> Result<(Arc<[u8]>, Digest, bool, Vec<String>), NativeError> {
        let (archive, digest, hit, _, link_arguments) =
            self.cached_number_provider_for(target, ProviderCapabilities::FULL)?;
        Ok((archive, digest, hit, link_arguments))
    }

    /// Resolve and cache the smallest provider variant covering `required`. The last element is
    /// what the archive needs on the link line beyond the C runtime's own libraries.
    fn cached_number_provider_for(
        &self,
        target: &str,
        required: ProviderCapabilities,
    ) -> Result<(Arc<[u8]>, Digest, bool, ProviderCapabilities, Vec<String>), NativeError> {
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
        let link_arguments = provider.link_arguments.clone();
        match self.lookup_digest(&provider_key)? {
            Some((digest, _size)) => {
                debug_assert_eq!(digest, provider.archive_digest);
                Ok((
                    Arc::clone(&provider.archive),
                    digest,
                    true,
                    capabilities,
                    link_arguments,
                ))
            }
            None => {
                let stored = self.cache.put(&provider_key, &provider.archive)?;
                Ok((
                    Arc::clone(&provider.archive),
                    stored.digest,
                    false,
                    capabilities,
                    link_arguments,
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
        match self.build_until(program, options, BuildUntil::Executable)? {
            Built::Executable(report) => Ok(report),
            Built::Emitted(_) => unreachable!("a build to the executable returns one"),
        }
    }

    /// Every `parallelMap` site [`Self::build`] would lower and what it decided, without
    /// compiling or linking: `kiln check --explain-parallel`. The build itself runs up to the
    /// end of emission (through the same generated-C cache) and stops there, so the answer is
    /// the one `kiln build --explain-parallel` reads off the C it compiles, by construction.
    pub fn parallel_sites(
        &self,
        program: &Program,
        options: &NativeBuildOptions,
    ) -> Result<ParallelSites, NativeError> {
        match self.build_until(program, options, BuildUntil::Emitted)? {
            Built::Emitted(sites) => Ok(sites),
            Built::Executable(report) => Ok(report.parallel_sites),
        }
    }

    fn build_until(
        &self,
        program: &Program,
        options: &NativeBuildOptions,
        until: BuildUntil,
    ) -> Result<Built, NativeError> {
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
        emission.parallel_in_generic = parallel_in_generic(options.profile);
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
        let parallel_sites: Vec<(String, Vec<kiln_backend_c::parallel::SiteRecord>)> = {
            let sites = parallel_sites_of(&module);
            if sites.is_empty() {
                Vec::new()
            } else {
                vec![("<main>".to_owned(), sites)]
            }
        };
        if until == BuildUntil::Emitted {
            return Ok(Built::Emitted(parallel_sites));
        }
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
        let (
            number_provider,
            number_provider_digest,
            _,
            provider_capabilities,
            provider_link_arguments,
        ) = self.cached_number_provider_for(&target, required_provider_capabilities)?;

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

        let march_opt = object_flag_identity(options);
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
                return Ok(Built::Executable(NativeBuildReport {
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
                    parallel_sites,
                }));
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
                    materialize_provider_archive(
                        &self.cache,
                        &paths.number_provider_a,
                        &number_provider,
                        number_provider_digest,
                    )?;
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
                    link_options
                        .extra_args
                        .extend(provider_link_arguments.iter().map(std::ffi::OsString::from));
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

        Ok(Built::Executable(NativeBuildReport {
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
            parallel_sites,
        }))
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
        emission.parallel_in_generic = parallel_in_generic(options.profile);
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
        let (number_provider, number_provider_digest, _, provider_link_arguments) =
            self.cached_number_provider(&target)?;
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
        let march_opt = object_flag_identity(options);
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
                materialize_provider_archive(
                    &self.cache,
                    &paths.number_provider_a,
                    &number_provider,
                    number_provider_digest,
                )?;
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
                link_options
                    .extra_args
                    .extend(provider_link_arguments.iter().map(std::ffi::OsString::from));
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
        let mut demand = kiln_liveness::executable_demand(
            graph,
            &selection_options_with_exclusions(&NativeProfile::Dev, &native_plan.excluded),
        )
        .map_err(NativeError::Graph)?;
        include_runtime_aliases_in_demand(graph, &mut demand, &native_plan.runtime_alias_live);
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
        emission.parallel_in_generic = parallel_in_generic(options.profile);
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
        let cyclic_modules = graph::preinstantiated_module_paths(graph);
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
                options.profile,
                cyclic_modules.contains(&node.path),
                || {
                    emit_graph_module_for_profile(
                        node,
                        &logical_path,
                        &module_emission,
                        options.profile,
                        cyclic_modules.contains(&node.path),
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
            Some(&demand.live_imports),
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
            if let Some(workers) = &module.parallel_workers {
                // Issue 14: the parallel workers of a generic-only module, compiled at the
                // optimising level of a specialized module. Its own source digest keys it; a
                // module with no qualifying site has none, and its units are unchanged.
                symbol_list.push(format!("{}_parallel_workers", module.symbol));
                module_sources.push((
                    workers.as_bytes().to_vec(),
                    Digest::hash(workers.as_bytes()),
                    hit,
                ));
                module_generic_only.push(false);
            }
            if let Some(instantiation) = &module.instantiation {
                symbol_list.push(instantiation.symbol.clone());
                module_sources.push((
                    instantiation.source.as_bytes().to_vec(),
                    Digest::hash(instantiation.source.as_bytes()),
                    hit,
                ));
                module_generic_only.push(module.generic_only);
            }
        }
        let paths = SharedGraphLibraryPaths::new(workspace.path(), &symbol_list);
        let profile = options.profile;
        let embedding_abi = runtime_abi_identity();
        let compiler = compiler_identity(&identity);
        let (number_provider, number_provider_digest, _, provider_link_arguments) =
            self.cached_number_provider(&target)?;
        let wrapper_source_key = NativeArtifactFamily::SharedGraphWrapperSource.source_key(
            &target,
            Digest::hash(b"deterministic-c11-shared-graph-wrapper-owned-result-v2"),
            [Digest::hash(wrapper.as_bytes()), topology],
        );
        let (wrapper_bytes, wrapper_source_digest, wrapper_generated_c_hit) =
            self.cached_payload(&wrapper_source_key, wrapper.as_bytes())?;

        let march_opt = object_flag_identity(options);
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
                materialize_provider_archive(
                    &self.cache,
                    &paths.number_provider_a,
                    &number_provider,
                    number_provider_digest,
                )?;
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
                link_options
                    .extra_args
                    .extend(provider_link_arguments.iter().map(std::ffi::OsString::from));
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
        match self.build_graph_until(graph, options, BuildUntil::Executable)? {
            Built::Executable(report) => Ok(report),
            Built::Emitted(_) => unreachable!("a build to the executable returns one"),
        }
    }

    /// [`Self::parallel_sites`] for a module graph: the same selection, reduction and
    /// emission as [`Self::build_graph`], stopped before anything is compiled.
    pub fn parallel_sites_graph(
        &self,
        graph: &ModuleGraph,
        options: &NativeBuildOptions,
    ) -> Result<ParallelSites, NativeError> {
        match self.build_graph_until(graph, options, BuildUntil::Emitted)? {
            Built::Emitted(sites) => Ok(sites),
            Built::Executable(report) => Ok(report.parallel_sites),
        }
    }

    fn build_graph_until(
        &self,
        graph: &ModuleGraph,
        options: &NativeBuildOptions,
        until: BuildUntil,
    ) -> Result<Built, NativeError> {
        let native_plan = native_graph_plan(graph)?;
        let total_started = Instant::now();
        let emit_started = Instant::now();
        let trace_emit = std::env::var_os("KILN_NATIVE_TRACE_EMIT").is_some();
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
        emission.parallel_in_generic = parallel_in_generic(options.profile);
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
        let initial_demand = selected.demand();
        let initial_members = kiln_graph::analyze_demanded_members(
            graph,
            &initial_demand.live,
            &initial_demand.slots,
        );
        let needs_http_facade = needs_native_http_facade(&initial_members);
        // Keep the optional augmented graph in this scope before borrowing it for selection.
        // A SelectedModules value borrows its graph and must never travel beside a moved owner.
        let augmented_graph = if needs_http_facade {
            // The separately compiled web bootstrap's dependency is invisible to source graph
            // discovery. Retain the public facade through a lazy runtime edge before selection.
            Some(
                graph
                    .with_lazy_node_builtin(
                        graph.entry(),
                        "/kiln/runtime-bootstrap.ts",
                        NodeBuiltin::Http,
                    )
                    .map_err(NativeError::Graph)?,
            )
        } else {
            None
        };
        let graph = augmented_graph.as_ref().unwrap_or(graph);
        let native_plan = if augmented_graph.is_some() {
            native_graph_plan(graph)?
        } else {
            native_plan
        };
        let selection = selection_options_with_exclusions(&options.profile, &native_plan.excluded);
        let selected = if augmented_graph.is_some() {
            kiln_liveness::select_executable_modules(graph, &selection)
                .map_err(NativeError::Graph)?
        } else {
            selected
        };
        // One shaken graph feeds emission: reduce to the live subset first,
        // so the emitter and launcher below only ever see live modules and
        // live edges. With everything live this is the identity transform.
        let mut demand = selected.demand();
        include_runtime_aliases_in_demand(graph, &mut demand, &native_plan.runtime_alias_live);
        debug_module_demands("selected", graph, &demand);
        let (reduced, demand) = reduced_graph(graph, &demand)?;
        debug_module_demands("reduced", &reduced, &demand);
        let (reduced, demand) = if matches!(
            options.profile,
            NativeProfile::Release | NativeProfile::ReleaseSize
        ) {
            let (reduced, demand) = dce_release_graph_to_fixpoint(reduced, demand, &selection)?;
            debug_module_demands("release-dce", &reduced, &demand);
            (reduced, demand)
        } else {
            // Non-release: re-parse for native import.meta without DCE.
            let (reduced, demand) = native_import_meta_reparse(&reduced, &demand);
            debug_module_demands("import-meta", &reduced, &demand);
            (reduced, demand)
        };
        let graph = &reduced;
        // Dispatch-group mask for the prelude: same closed-world selection,
        // so the `-D` flags below only ever remove what nothing retained can
        // reach. Computed once; the explain path recomputes it out-of-band.
        let members = kiln_graph::analyze_demanded_members(graph, &demand.live, &demand.slots);
        let group_mask = kiln_liveness::retained_method_groups(graph, &demand, &members)
            | graph::NATIVE_WEB_BOOTSTRAP_METHOD_GROUPS;
        if trace_emit {
            eprintln!(
                "kiln: graph selection and member analysis {}ms",
                emit_started.elapsed().as_millis()
            );
        }
        let nodes: Vec<&kiln_module_graph::ModuleNode> = graph.modules().collect();
        let cyclic_modules = graph::preinstantiated_module_paths(graph);
        let emitted_modules = emit_indexed(nodes.len(), "graph module emission", |index| {
            let module_started = trace_emit.then(Instant::now);
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
                options.profile,
                cyclic_modules.contains(&node.path),
                || {
                    emit_graph_module_for_profile(
                        node,
                        &logical_path,
                        &module_emission,
                        options.profile,
                        cyclic_modules.contains(&node.path),
                    )
                },
            )?;
            if let Some(started) = module_started {
                eprintln!(
                    "kiln: module emission {}ms {} generated-C {}",
                    started.elapsed().as_millis(),
                    logical_path,
                    if generated_c_hit { "hit" } else { "miss" }
                );
            }
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
        let parallel_sites: Vec<(String, Vec<kiln_backend_c::parallel::SiteRecord>)> = modules
            .iter()
            .zip(&logical_paths)
            .filter_map(|((_, module), path)| {
                let sites = parallel_sites_of(module);
                (!sites.is_empty()).then(|| (path.clone(), sites))
            })
            .collect();
        if until == BuildUntil::Emitted {
            return Ok(Built::Emitted(parallel_sites));
        }
        let bootstrap_started = trace_emit.then(Instant::now);
        let (bootstrap, _bootstrap_program, bootstrap_capabilities, bootstrap_generated_c_hit) =
            native_builtin_bootstrap_module(
                &self.cache,
                &target,
                &emission,
                options.node,
                group_mask & kiln_graph::method_groups::URL != 0,
                graph::native_bootstrap_families(&members, options.node, group_mask),
                (options.profile == NativeProfile::Release && release_bootstrap_specialized())
                    .then_some(RELEASE_BOOTSTRAP_UNIT_BYTES),
            )?;
        if let Some(started) = bootstrap_started {
            eprintln!(
                "kiln: bootstrap emission {}ms generated-C {}",
                started.elapsed().as_millis(),
                if bootstrap_generated_c_hit {
                    "hit"
                } else {
                    "miss"
                }
            );
        }
        if !symbols.insert(bootstrap.symbol.clone()) {
            return Err(NativeError::SymbolCollision {
                symbol: bootstrap.symbol,
            });
        }
        let launcher_started = trace_emit.then(Instant::now);
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
        if let Some(started) = launcher_started {
            eprintln!(
                "kiln: launcher emission {}ms",
                started.elapsed().as_millis()
            );
        }
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
            if let Some(workers) = &module.parallel_workers {
                // Issue 14: the parallel workers of a generic-only module, compiled at the
                // optimising level of a specialized module. Its own source digest keys it; a
                // module with no qualifying site has none, and its units are unchanged.
                symbol_list.push(format!("{}_parallel_workers", module.symbol));
                module_sources.push((
                    workers.as_bytes().to_vec(),
                    Digest::hash(workers.as_bytes()),
                    hit,
                ));
                module_generic_only.push(false);
            }
            if let Some(instantiation) = &module.instantiation {
                symbol_list.push(instantiation.symbol.clone());
                module_sources.push((
                    instantiation.source.as_bytes().to_vec(),
                    Digest::hash(instantiation.source.as_bytes()),
                    hit,
                ));
                module_generic_only.push(module.generic_only);
            }
            module_chunk_ranges.push(start..symbol_list.len());
        }
        // The bootstrap's translation units: its source, or its chunks when a release build split
        // its specialized C (issue 17), each compiled like an ordinary module's chunk.
        let bootstrap_units: Vec<(String, &str)> = if bootstrap.chunks.is_empty() {
            vec![(bootstrap.symbol.clone(), bootstrap.source.as_str())]
        } else {
            bootstrap
                .chunks
                .iter()
                .enumerate()
                .map(|(index, chunk)| {
                    (
                        format!("{}_chunk_{index}", bootstrap.symbol),
                        chunk.as_str(),
                    )
                })
                .collect()
        };
        symbol_list.extend(bootstrap_units.iter().map(|(name, _)| name.clone()));
        let paths = GraphBuildPaths::new(workspace.path(), &symbol_list);
        let profile = options.profile;
        let compiler = compiler_identity(&identity);
        let required_provider_capabilities =
            kiln_liveness::retained_provider_capabilities(graph, &demand, &members)
                .union(bootstrap_capabilities);
        let (
            number_provider,
            number_provider_digest,
            _,
            provider_capabilities,
            provider_link_arguments,
        ) = self.cached_number_provider_for(&target, required_provider_capabilities)?;

        for (_, unit) in &bootstrap_units {
            let bootstrap_source_key = NativeArtifactFamily::ModuleSource.source_key(
                &target,
                Digest::hash(b"native-builtin-bootstrap-c11"),
                [Digest::hash(unit.as_bytes())],
            );
            let (bootstrap_bytes, bootstrap_digest, _) =
                self.cached_payload(&bootstrap_source_key, unit.as_bytes())?;
            module_sources.push((bootstrap_bytes, bootstrap_digest, bootstrap_generated_c_hit));
            module_generic_only.push(bootstrap.generic_only);
        }
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

        let march_opt = object_flag_identity(options);
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
                return Ok(Built::Executable(NativeBuildReport {
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
                    parallel_sites,
                }));
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
                materialize_provider_archive(
                    &self.cache,
                    &paths.number_provider_a,
                    &number_provider,
                    number_provider_digest,
                )?;
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
                link_options
                    .extra_args
                    .extend(provider_link_arguments.iter().map(std::ffi::OsString::from));
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
        Ok(Built::Executable(NativeBuildReport {
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
            parallel_sites,
        }))
    }

    /// Build and execute a module graph with captured output.
    pub fn run_graph(
        &self,
        graph: &ModuleGraph,
        options: &NativeBuildOptions,
    ) -> Result<NativeRunReport, NativeError> {
        self.run_graph_streaming(graph, options, None)
    }

    /// As [`Self::run_graph`], but with `on_output` the program's output is delivered as it is
    /// written and the report's `stdout`/`stderr` are empty. A program that never exits, such as
    /// a server, has no other way to say anything.
    pub fn run_graph_streaming(
        &self,
        graph: &ModuleGraph,
        options: &NativeBuildOptions,
        on_output: Option<&mut dyn FnMut(ChildStream, &[u8])>,
    ) -> Result<NativeRunReport, NativeError> {
        self.run_graph_with_output_mode(graph, options, on_output, false)
    }

    /// Build and execute with the caller's stdout and stderr descriptors inherited by the child.
    pub fn run_graph_inheriting_output(
        &self,
        graph: &ModuleGraph,
        options: &NativeBuildOptions,
    ) -> Result<NativeRunReport, NativeError> {
        self.run_graph_with_output_mode(graph, options, None, true)
    }

    fn run_graph_with_output_mode(
        &self,
        graph: &ModuleGraph,
        options: &NativeBuildOptions,
        on_output: Option<&mut dyn FnMut(ChildStream, &[u8])>,
        inherit_output: bool,
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
            on_output,
            inherit_output,
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
        self.run_streaming(program, options, None)
    }

    /// As [`Self::run`], delivering output through `on_output` as in [`Self::run_graph_streaming`].
    pub fn run_streaming(
        &self,
        program: &Program,
        options: &NativeBuildOptions,
        on_output: Option<&mut dyn FnMut(ChildStream, &[u8])>,
    ) -> Result<NativeRunReport, NativeError> {
        self.run_with_output_mode(program, options, on_output, false)
    }

    /// Build and execute with the caller's stdout and stderr descriptors inherited by the child.
    /// This is for an in-process one-shot CLI; daemon clients must use [`Self::run_streaming`].
    pub fn run_inheriting_output(
        &self,
        program: &Program,
        options: &NativeBuildOptions,
    ) -> Result<NativeRunReport, NativeError> {
        self.run_with_output_mode(program, options, None, true)
    }

    fn run_with_output_mode(
        &self,
        program: &Program,
        options: &NativeBuildOptions,
        on_output: Option<&mut dyn FnMut(ChildStream, &[u8])>,
        inherit_output: bool,
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
            on_output,
            inherit_output,
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

    fn object_compile_lock_file(&self, bucket: usize) -> Result<(fs::File, PathBuf), NativeError> {
        let layer = self
            .cache
            .layers()
            .iter()
            .find(|layer| layer.is_writable())
            .ok_or(CacheError::NoWritableLayer)?;
        let directory = layer.root().join(".compile-locks/v1");
        fs::create_dir_all(&directory).map_err(|source| NativeError::Io {
            operation: "create native object compile lock directory",
            path: directory.clone(),
            source,
        })?;
        let path = directory.join(format!("{bucket:02x}.lock"));
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| NativeError::Io {
                operation: "open native object compile lock",
                path: path.clone(),
                source,
            })?;
        Ok((file, path))
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
                let file_bucket = usize::from(key.digest().as_bytes()[0]);
                let (lock_file, lock_path) = self.object_compile_lock_file(file_bucket)?;
                let canonical_lock_path =
                    fs::canonicalize(&lock_path).map_err(|source| NativeError::Io {
                        operation: "resolve native object compile lock",
                        path: lock_path.clone(),
                        source,
                    })?;
                let mut hasher = DefaultHasher::new();
                canonical_lock_path.hash(&mut hasher);
                let process_bucket =
                    (hasher.finish() % OBJECT_COMPILE_LOCK_BUCKETS as u64) as usize;
                let _process_lock = OBJECT_COMPILE_LOCKS[process_bucket].lock();
                lock_file.lock().map_err(|source| NativeError::Io {
                    operation: "lock native object compile bucket",
                    path: lock_path,
                    source,
                })?;

                // A competing process or thread may have published this exact full key while we
                // waited. Bucket collisions only cause extra waiting; this lookup remains the
                // authority for reuse and preserves the cache's corruption diagnostics.
                match self.lookup(key)? {
                    Lookup::Hit(artifact) => {
                        Ok((artifact.bytes, artifact.digest, true, Duration::ZERO))
                    }
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
    }
}

mod graph;
use graph::*;
mod bootstrap_emitter;
mod graph_source_emitter;
use graph_source_emitter::parallel_in_generic;
#[cfg(test)]
use graph_source_emitter::{
    MAX_RELEASE_SPECIALIZED_MODULE_FUNCTIONS, MAX_RELEASE_SPECIALIZED_MODULE_INSTRUCTIONS,
    release_module_uses_generic_bodies,
};
mod shared_graph;
use shared_graph::*;

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
