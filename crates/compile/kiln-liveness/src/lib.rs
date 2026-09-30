//! Closed-world liveness: which modules, exports, import edges, and dispatch
//! groups one executable retains.
//!
//! The selection walk used to live in `kiln-native` beside the emitter it
//! served. It moves here, over the same inputs, so the interpreter side can
//! reduce a [`ModuleGraph`] to the same live subset the native backend emits:
//! one shaken graph feeds both engines. [`select_executable_modules`]
//! chooses, [`LiveDemand`] (in `kiln-module-graph`) carries, and
//! [`ModuleGraph::reduce_to_live`] consumes.
//!
//! [`ModuleGraph::reduce_to_live`]: kiln_module_graph::ModuleGraph::reduce_to_live

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use kiln_graph::{
    CORE_METHOD_GROUPS, DemandedMembers, PRUNABLE_METHOD_GROUPS,
    closed_world_module_live_slots_with_declarations, closed_world_namespace_members,
    method_groups as groups, retained_module_instructions,
};
use kiln_module_graph::{
    Dependency, LiveDemand, ModuleGraph, ModuleGraphError, ModuleNode, ModuleSource, ResolvedExport,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModuleSizeFacts {
    pub path: String,
    pub live: bool,
    pub total_instructions: usize,
    pub total_functions: usize,
    pub retained_instructions: usize,
    pub retained_functions: usize,
}

/// What the selection may assume about its consumer.
///
/// Release-style production dispatch drops the development arm of the exact two-arm
/// React-style CommonJS dispatcher. Backend capability exclusions omit unsupported dynamic
/// closures while callers retain their deferred failure records.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SelectionOptions {
    /// Drop the inactive arm of the two-arm production dispatcher.
    pub production_dispatch: bool,
    /// Modules rejected by a backend capability plan, typically unsupported dynamic-import
    /// closures. They remain represented by the caller's deferred failure table and are omitted
    /// from the emitted graph.
    pub excluded: BTreeSet<PathBuf>,
}

impl SelectionOptions {
    /// Selection for a shipped release-style build.
    pub const fn production() -> Self {
        Self {
            production_dispatch: true,
            excluded: BTreeSet::new(),
        }
    }

    /// Selection for development, measurement, and interpreter reduction.
    pub const fn development() -> Self {
        Self {
            production_dispatch: false,
            excluded: BTreeSet::new(),
        }
    }
}

#[derive(Debug)]
pub struct ExecutableSelection<'a> {
    /// Live modules in dependency-first order.
    pub modules: Vec<&'a ModuleNode>,
    /// Live import edges as `(importer, index into its `program.imports`)`.
    pub live_imports: BTreeSet<(PathBuf, usize)>,
    /// Demanded export slots per live module.
    pub live_export_slots: BTreeMap<PathBuf, BTreeSet<u32>>,
    /// React-style development-arm edges to skip, release only.
    pub inactive_commonjs_dependencies: BTreeSet<(PathBuf, String)>,
    /// Statically known namespace members per live module.
    pub partial_namespace_members: BTreeMap<PathBuf, BTreeSet<String>>,
}

/// The exact two-arm dispatcher used by React and similar CommonJS packages.
///
/// This intentionally accepts no general JavaScript. Matching the complete whitespace-free
/// source makes the specialization semantic rather than filename-based: in a release realm the
/// production arm is the only arm that can run, while any extra statement, condition, or third
/// dependency falls back to the complete graph.
fn commonjs_production_dispatch(
    source: &str,
    dependencies: &[Dependency],
) -> Option<(String, String)> {
    if dependencies.len() != 2 {
        return None;
    }
    let compact = source
        .chars()
        .filter(|character| !character.is_whitespace())
        .map(|character| if character == '"' { '\'' } else { character })
        .collect::<String>();
    for (production, development) in [
        (&dependencies[0], &dependencies[1]),
        (&dependencies[1], &dependencies[0]),
    ] {
        let expected = format!(
            "'usestrict';if(process.env.NODE_ENV==='production'){{module.exports=require('{}');}}else{{module.exports=require('{}');}}",
            production.specifier, development.specifier
        );
        if compact == expected {
            return Some((production.specifier.clone(), development.specifier.clone()));
        }
    }
    None
}

/// Whether the nearest package boundary explicitly licenses removing unused modules.
///
/// Absence, malformed metadata, and non-empty pattern lists all remain conservative. Supporting
/// package-relative `sideEffects` globs needs the resolver to preserve package identity, so this
/// first closed-world optimization only accepts the two unambiguous all-files spellings.
fn package_declares_no_side_effects(path: &Path, project_root: &Path) -> bool {
    let mut directory = path.parent();
    while let Some(current) = directory {
        if !current.starts_with(project_root) {
            break;
        }
        let manifest = current.join("package.json");
        if manifest.is_file() {
            let Ok(bytes) = fs::read(&manifest) else {
                return false;
            };
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                return false;
            };
            return matches!(
                value.get("sideEffects"),
                Some(serde_json::Value::Bool(false))
            ) || matches!(value.get("sideEffects"), Some(serde_json::Value::Array(items)) if items.is_empty());
        }
        if current == project_root {
            break;
        }
        directory = current.parent();
    }
    false
}

fn demand_namespace_members(
    graph: &ModuleGraph,
    namespace: &Path,
    members: &BTreeSet<String>,
    live: &mut BTreeSet<PathBuf>,
    live_export_slots: &mut BTreeMap<PathBuf, BTreeSet<u32>>,
    namespace_demands: &mut BTreeSet<PathBuf>,
    queue: &mut VecDeque<PathBuf>,
) -> Result<(), ModuleGraphError> {
    for name in members {
        match graph.resolved_export(namespace, name) {
            Some(ResolvedExport::Local { module, local_slot }) => {
                let newly_live = live.insert(module.clone());
                let newly_demanded = live_export_slots
                    .entry(module.clone())
                    .or_default()
                    .insert(*local_slot);
                if newly_live || newly_demanded {
                    queue.push_back(module.clone());
                }
            }
            Some(ResolvedExport::CommonJsNamed { module, .. }) => {
                let newly_live = live.insert(module.clone());
                let newly_demanded = live_export_slots
                    .entry(module.clone())
                    .or_default()
                    .insert(0);
                if newly_live || newly_demanded {
                    queue.push_back(module.clone());
                }
            }
            Some(ResolvedExport::Namespace { module }) => {
                namespace_demands.insert(module.clone());
                if live.insert(module.clone()) {
                    queue.push_back(module.clone());
                }
            }
            Some(ResolvedExport::Ambiguous) | None => {
                if graph
                    .module(namespace)
                    .is_some_and(|node| node.source == ModuleSource::CommonJs)
                {
                    let newly_live = live.insert(namespace.to_path_buf());
                    let newly_demanded = live_export_slots
                        .entry(namespace.to_path_buf())
                        .or_default()
                        .insert(0);
                    if newly_live || newly_demanded {
                        queue.push_back(namespace.to_path_buf());
                    }
                } else {
                    return Err(ModuleGraphError::MissingExport {
                        importer: namespace.to_path_buf(),
                        provider: namespace.to_path_buf(),
                        name: name.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Select the source bodies an executable can observe after applying package side-effect facts.
///
/// The complete graph is still linked first, so missing and ambiguous exports remain errors. This
/// pass only removes evaluation work that package metadata explicitly declares unobservable.
pub fn select_executable_modules<'a>(
    graph: &'a ModuleGraph,
    options: &SelectionOptions,
) -> Result<ExecutableSelection<'a>, ModuleGraphError> {
    let mut live = BTreeSet::new();
    let mut live_imports = BTreeSet::new();
    let mut live_export_slots: BTreeMap<PathBuf, BTreeSet<u32>> = BTreeMap::new();
    let mut namespace_demands = BTreeSet::new();
    let mut namespace_member_cache = BTreeMap::new();
    let mut partial_namespace_members: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    let mut inactive_commonjs_dependencies = BTreeSet::new();
    let mut queue = VecDeque::new();
    let mut side_effect_free = BTreeMap::new();

    let mark_live = |path: &Path, live: &mut BTreeSet<PathBuf>, queue: &mut VecDeque<PathBuf>| {
        if options.excluded.contains(path) {
            return;
        }
        if live.insert(path.to_path_buf()) {
            queue.push_back(path.to_path_buf());
        }
    };
    mark_live(graph.entry(), &mut live, &mut queue);
    // Declared includes are potential runtime entries (for example a worker's target supplied
    // in workerData). They are linked and retained, but entry evaluation still starts only from
    // graph.entry(). A loader may ask for any export, so preserve each complete namespace.
    for path in graph.declared_includes() {
        namespace_demands.insert(path.clone());
        mark_live(path, &mut live, &mut queue);
    }

    let has_any_dynamic_import = graph
        .modules()
        .any(|m| m.program.program.has_dynamic_import());
    if has_any_dynamic_import {
        for m in graph.modules() {
            if options.excluded.contains(&m.path) {
                continue;
            }
            namespace_demands.insert(m.path.clone());
            mark_live(&m.path, &mut live, &mut queue);
        }
    }

    while let Some(path) = queue.pop_front() {
        let node = graph
            .module(&path)
            .ok_or_else(|| ModuleGraphError::InternalMissingModule { path: path.clone() })?;
        let direct_eval = node.program.link_facts.direct_eval;
        let demanded = live_export_slots.get(&path).cloned().unwrap_or_default();
        let live_slots = closed_world_module_live_slots_with_declarations(
            &node.program.program,
            &demanded,
            direct_eval,
            &node.program.link_facts.removable_declarations,
        );

        let production_dispatch = options
            .production_dispatch
            .then(|| commonjs_production_dispatch(&node.program.program.source, &node.dependencies))
            .flatten();
        for dependency in &node.dependencies {
            if production_dispatch
                .as_ref()
                .is_some_and(|(_, inactive)| inactive == &dependency.specifier)
            {
                inactive_commonjs_dependencies
                    .insert((node.path.clone(), dependency.specifier.clone()));
                continue;
            }
            let removable = *side_effect_free
                .entry(dependency.target.clone())
                .or_insert_with(|| {
                    package_declares_no_side_effects(&dependency.target, graph.project_root())
                });
            if node.source == ModuleSource::CommonJs || !removable {
                mark_live(&dependency.target, &mut live, &mut queue);
                if node.source == ModuleSource::CommonJs
                    && graph
                        .module(&dependency.target)
                        .is_some_and(|target| target.source != ModuleSource::CommonJs)
                {
                    // CommonJS receives the complete namespace from synchronous require(esm),
                    // so every ESM export remains observable across this edge.
                    if namespace_demands.insert(dependency.target.clone()) {
                        queue.push_back(dependency.target.clone());
                    }
                }
            }
        }
        // Literal createRequire/getBuiltinModule targets are lazy at execution time but remain
        // part of the closed graph: the runtime loader may request any one of them.
        for dependency in &node.runtime_dependencies {
            mark_live(&dependency.target, &mut live, &mut queue);
            // A lazy require returns the target's complete runtime value. Preserve its CommonJS
            // module.exports slot or, for ESM, demand the full namespace before reduction.
            namespace_demands.insert(dependency.target.clone());
        }

        for (import_index, import) in node.program.imports.iter().enumerate() {
            if node.source == ModuleSource::CommonJs && import.source == "*commonjs-wrapper*" {
                continue;
            }
            let dependency = node
                .dependencies
                .iter()
                .find(|dependency| dependency.specifier == import.source)
                .ok_or_else(|| ModuleGraphError::UnresolvedImport {
                    importer: node.path.clone(),
                    source: import.source.clone(),
                })?;
            if !live_slots.contains(&import.local_slot) {
                continue;
            }
            live_imports.insert((node.path.clone(), import_index));
            if import.imported == kiln_ir::NAMESPACE_IMPORT {
                let static_members = namespace_member_cache
                    .entry((node.path.clone(), import.local_slot))
                    .or_insert_with(|| {
                        closed_world_namespace_members(
                            &node.program.program,
                            import.local_slot,
                            direct_eval,
                        )
                    })
                    .clone();
                if let Some(members) = static_members {
                    mark_live(&dependency.target, &mut live, &mut queue);
                    partial_namespace_members
                        .entry(dependency.target.clone())
                        .or_default()
                        .extend(members.iter().cloned());
                    demand_namespace_members(
                        graph,
                        &dependency.target,
                        &members,
                        &mut live,
                        &mut live_export_slots,
                        &mut namespace_demands,
                        &mut queue,
                    )?;
                } else {
                    namespace_demands.insert(dependency.target.clone());
                    mark_live(&dependency.target, &mut live, &mut queue);
                }
                continue;
            }
            match graph.resolved_export(&dependency.target, &import.imported) {
                Some(ResolvedExport::Local { module, local_slot }) => {
                    let newly_demanded = live_export_slots
                        .entry(module.clone())
                        .or_default()
                        .insert(*local_slot);
                    mark_live(module, &mut live, &mut queue);
                    if newly_demanded {
                        queue.push_back(module.clone());
                    }
                }
                Some(ResolvedExport::CommonJsNamed { module, .. }) => {
                    let newly_demanded = live_export_slots
                        .entry(module.clone())
                        .or_default()
                        .insert(0);
                    mark_live(module, &mut live, &mut queue);
                    if newly_demanded {
                        queue.push_back(module.clone());
                    }
                }
                Some(ResolvedExport::Namespace { module }) => {
                    let static_members = namespace_member_cache
                        .entry((node.path.clone(), import.local_slot))
                        .or_insert_with(|| {
                            closed_world_namespace_members(
                                &node.program.program,
                                import.local_slot,
                                direct_eval,
                            )
                        })
                        .clone();
                    if let Some(members) = static_members {
                        mark_live(module, &mut live, &mut queue);
                        partial_namespace_members
                            .entry(module.clone())
                            .or_default()
                            .extend(members.iter().cloned());
                        demand_namespace_members(
                            graph,
                            module,
                            &members,
                            &mut live,
                            &mut live_export_slots,
                            &mut namespace_demands,
                            &mut queue,
                        )?;
                    } else {
                        namespace_demands.insert(module.clone());
                        mark_live(module, &mut live, &mut queue);
                    }
                }
                Some(ResolvedExport::Ambiguous) | None => {
                    return Err(ModuleGraphError::MissingExport {
                        importer: node.path.clone(),
                        provider: dependency.target.clone(),
                        name: import.imported.clone(),
                    });
                }
            }
        }

        if namespace_demands.contains(&path) {
            if node.source == ModuleSource::CommonJs {
                let newly_demanded = live_export_slots.entry(path.clone()).or_default().insert(0);
                mark_live(&path, &mut live, &mut queue);
                if newly_demanded {
                    queue.push_back(path.clone());
                }
                continue;
            }
            let names = graph
                .resolved_export_names(&path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule { path: path.clone() })?;
            for name in names {
                match graph.resolved_export(&path, &name) {
                    Some(ResolvedExport::Local { module, local_slot }) => {
                        let newly_demanded = live_export_slots
                            .entry(module.clone())
                            .or_default()
                            .insert(*local_slot);
                        mark_live(module, &mut live, &mut queue);
                        if newly_demanded {
                            queue.push_back(module.clone());
                        }
                    }
                    Some(ResolvedExport::CommonJsNamed { module, .. }) => {
                        let newly_demanded = live_export_slots
                            .entry(module.clone())
                            .or_default()
                            .insert(0);
                        mark_live(module, &mut live, &mut queue);
                        if newly_demanded {
                            queue.push_back(module.clone());
                        }
                    }
                    Some(ResolvedExport::Namespace { module }) => {
                        // An observable namespace re-export exposes the provider's complete
                        // namespace. Marking only the provider module live leaves its export
                        // slots empty, so downstream namespace objects can contain missing
                        // members (for example htmlparser2's `DomUtils.getElementById`).
                        let newly_live = live.insert(module.clone());
                        let newly_demanded = namespace_demands.insert(module.clone());
                        if newly_live || newly_demanded {
                            queue.push_back(module.clone());
                        }
                    }
                    Some(ResolvedExport::Ambiguous) | None => {}
                }
            }
        }
    }

    // Namespace requirements are joined across every importer. One dynamic/escaping use requires
    // the complete namespace even when another importer has a statically known subset.
    partial_namespace_members.retain(|path, _| !namespace_demands.contains(path));

    Ok(ExecutableSelection {
        modules: graph
            .modules()
            .filter(|node| live.contains(&node.path))
            .collect(),
        live_imports,
        live_export_slots,
        inactive_commonjs_dependencies,
        partial_namespace_members,
    })
}

/// Closed-world demand for an executable graph: which modules are live and
/// which export slots each one must keep. Computed once by
/// [`executable_demand`] and shared by size attribution and member analysis
/// so both read the same selection the build emits.
///
/// [`select_executable_modules`] chooses, [`ExecutableSelection::demand`]
/// reshapes, and [`ModuleGraph::reduce_to_live`] consumes.
impl<'a> ExecutableSelection<'a> {
    /// The shared [`LiveDemand`] for this selection: live modules, demanded
    /// export slots, and live import edges.
    pub fn demand(&self) -> LiveDemand {
        LiveDemand {
            live: self.modules.iter().map(|node| node.path.clone()).collect(),
            slots: self.live_export_slots.clone(),
            live_imports: self.live_imports.clone(),
        }
    }
}

/// Run the closed-world selection and reshape it into the shared demand.
///
/// Pure query: no emission, no compilation. Backs `build --explain` size and
/// member attribution as well as graph reduction.
pub fn executable_demand(
    graph: &ModuleGraph,
    options: &SelectionOptions,
) -> Result<LiveDemand, ModuleGraphError> {
    Ok(select_executable_modules(graph, options)?.demand())
}

/// Method dispatch groups retained by one executable, as a bitmask over
/// `kiln_graph::method_groups`.
///
/// Core groups always; semantic and node-ish groups only on demand. A group is implicated
/// by an imported node builtin (conservative supersets: an HTTP server also
/// needs net/stream/event implementations), by the `Buffer`/`Process` globals
/// behind retained `LoadBuiltin`s, or by computed access (which can resolve
/// through `process` and `globalThis`). Direct eval, `new Function`,
/// indirect `eval`, and dynamic imports keep the full table: their targets
/// are unknowable. Successful builds resolve every static require, so CJS
/// literal requires are already covered by the graph edges below.
pub fn retained_method_groups(
    graph: &ModuleGraph,
    demand: &LiveDemand,
    members: &DemandedMembers,
) -> u32 {
    use kiln_module_resolver::NodeBuiltin;

    const FULL: u32 = CORE_METHOD_GROUPS | PRUNABLE_METHOD_GROUPS;

    // Builtin globals with group-backed methods behind them.
    let uses_buffer = members
        .builtins
        .iter()
        .any(|name| name.as_str() == "Buffer");
    // `process.stdout` is a stream and `process.on` an emitter: touching the
    // `process` global can reach both implementations without any import.
    let uses_process = members
        .builtins
        .iter()
        .any(|name| name.as_str() == "Process");
    // `process[k]()` and `globalThis[k]()` dynamics resolve through process
    // members (events/streams) and globals (`Buffer`); user objects implicate
    // nothing the router serves.
    let dynamic = members.has_computed_access;
    // Indirect `eval` runs arbitrary code.
    let indirect_eval = members.builtins.iter().any(|name| name.as_str() == "Eval");
    let reaches_global_object = members
        .builtins
        .iter()
        .any(|name| name.as_str() == "GlobalThis");
    let uses_temporal = members.globals.iter().any(|name| name == "Temporal");
    // `URL` is a builtin the emitter loads by name, `URLSearchParams` a global the bootstrap
    // defines; `fetch`, `Request` and `Response` are built on them under `--node`, and every
    // `node:` module that takes a URL string reduces it through `new URL`.
    let uses_url = members.builtins.iter().any(|name| name == "Url")
        || members.globals.iter().any(|name| {
            name == "URLSearchParams" || name == "fetch" || name == "Request" || name == "Response"
        });
    // `Bun.serve` and `Kiln.serve` are the bootstrap's `serve` (kiln-builtins web.ts), which
    // reaches the HTTP host through `process.__kilnNativeModules.http` rather than through an
    // import of `node:http` -- so the global is the only sign of it a program gives.
    let uses_serve_global = members
        .globals
        .iter()
        .any(|name| name == "Bun" || name == "Kiln");
    let uses_date = members.builtins.iter().any(|name| name == "Date");
    let uses_math_number = members
        .builtins
        .iter()
        .any(|name| name == "Math" || name == "Number");

    // Imported node builtins, from resolved graph edges: ESM imports and CJS
    // literal requires alike. Unknown `node:*` specifiers bail: failing closed
    // keeps a program working.
    fn builtin_groups(builtin: &NodeBuiltin) -> Option<u32> {
        use NodeBuiltin::*;
        Some(match builtin {
            Http | Https | Http2 => {
                groups::URL
                    | groups::NODE_HTTP
                    | groups::NODE_RESPONSE
                    | groups::NODE_SERVER
                    | groups::NODE_NET
                    | groups::NODE_STREAM
                    | groups::NODE_EVENT
                    | groups::BUFFER
            }
            Net => {
                groups::NODE_NET
                    | groups::NODE_SERVER
                    | groups::NODE_STREAM
                    | groups::NODE_EVENT
                    | groups::BUFFER
            }
            Events => groups::NODE_EVENT,
            Stream | StreamPromises | StreamWeb => {
                groups::NODE_STREAM | groups::NODE_EVENT | groups::BUFFER
            }
            Buffer => groups::BUFFER,
            Fs | FsPromises => groups::NODE_STREAM | groups::NODE_EVENT | groups::BUFFER,
            // Audited `Direct`-only (see phase1-PRD): no groups implicated.
            Url => groups::URL,
            Actors | ActorsCore | ActorsWorker | Assert | Console | Crypto | Dns | Module | Os
            | Process | Shared | Tty | Zlib | StringDecoder | QueryString | WorkerThreads
            | PerfHooks | AsyncHooks | ChildProcess | Timers | TimersPromises | Readline
            | ReadlinePromises | Sqlite | Constants | V8 | Vm | Path | Util | UtilTypes
            | DiagnosticsChannel | Gui => 0,
            // `dlopen` hands out raw pointers: anything is reachable.
            Ffi => return None,
        })
    }

    let mut mask = CORE_METHOD_GROUPS;
    if reaches_global_object {
        return FULL;
    }
    if uses_url || dynamic {
        mask |= groups::URL;
    }
    if uses_serve_global {
        mask |= builtin_groups(&NodeBuiltin::Http).unwrap_or(FULL);
    }
    if uses_temporal {
        mask |= groups::TEMPORAL
            | groups::TEMPORAL_FROM
            | groups::TEMPORAL_COMPARE
            | groups::TEMPORAL_NOW_READ
            | groups::TEMPORAL_OP;
    }
    if uses_date {
        mask |= groups::DATE_SETTER | groups::MATH_NUMBER;
    }
    if uses_math_number {
        mask |= groups::MATH_NUMBER;
    }
    if uses_buffer || dynamic {
        mask |= groups::BUFFER;
    }
    if uses_process || dynamic {
        mask |= groups::NODE_EVENT | groups::NODE_STREAM;
    }
    for node in graph.modules() {
        if !demand.live.contains(&node.path) {
            continue;
        }
        // Dynamic import targets and `new Function` bodies are unknowable.
        if !node.program.link_facts.dynamic_imports.is_empty()
            || node.program.program.has_dynamic_import()
            || node.program.link_facts.new_function
            || indirect_eval
        {
            return FULL;
        }
        for dependency in &node.dependencies {
            let target = graph.module(&dependency.target);
            match target.map(|module| &module.source) {
                Some(ModuleSource::NodeBuiltin(builtin)) => match builtin_groups(builtin) {
                    Some(groups) => mask |= groups,
                    None => return FULL,
                },
                // An explicit `node:` specifier that resolved anywhere else is
                // unexpected; failing closed keeps the program working.
                _ if dependency.specifier.starts_with("node:") => return FULL,
                _ => {}
            }
        }
        for dependency in &node.runtime_dependencies {
            let target = graph.module(&dependency.target);
            match target.map(|module| &module.source) {
                Some(ModuleSource::NodeBuiltin(builtin)) => match builtin_groups(builtin) {
                    Some(groups) => mask |= groups,
                    None => return FULL,
                },
                _ if dependency.specifier.starts_with("node:") => return FULL,
                _ => {}
            }
        }
    }
    // The members walk already bails the open world (direct eval, dynamic
    // import) into `has_open_world`; `new Function` and indirect eval are
    // checked above, so reaching here is closed.
    if members.has_open_world {
        return FULL;
    }
    // `Buffer.prototype` is the shared TypeScript (`kiln-builtins/src/buffer.ts`), and its
    // readers and writers truncate and range-check through `Math` and `Number`; a program that
    // reaches Buffer -- by the global or through `fs`, a stream or a server -- reaches those,
    // whether or not it spells them itself.
    if mask & groups::BUFFER != 0 {
        mask |= groups::MATH_NUMBER;
    }
    mask
}

/// Conservative capabilities required by a single module program.
pub fn program_provider_capabilities(
    module: &kiln_ir::ModuleProgram,
    demanded: Option<&BTreeSet<u32>>,
) -> kiln_native_provider_contract::ProviderCapabilities {
    use kiln_ir::{Builtin, Constant, Instruction};
    use kiln_native_provider_contract::{ProviderCapabilities, ProviderCapability};

    let program = &module.program;
    let mut required = ProviderCapabilities::NONE;
    let mut inspect = |instruction: &Instruction| {
        required = match instruction {
            Instruction::NewRegExp { .. } => required.with(ProviderCapability::RegExp),
            Instruction::LoadBuiltin(Builtin::RegExp) => required.with(ProviderCapability::RegExp),
            Instruction::LoadBuiltin(Builtin::BigInt) => required.with(ProviderCapability::BigInt),
            Instruction::LoadBuiltin(Builtin::Json) => required.with(ProviderCapability::Json),
            Instruction::Const(index)
                if matches!(
                    program.constants.get(*index as usize),
                    Some(Constant::BigInt(_))
                ) =>
            {
                required.with(ProviderCapability::BigInt)
            }
            _ => required,
        };
        let index = match instruction {
            Instruction::LoadGlobal(name)
            | Instruction::LoadGlobalBindingOrUndefined(name)
            | Instruction::RequireGlobalBinding(name)
            | Instruction::GetProperty(name)
            | Instruction::CallMethod { name, .. } => *name,
            _ => return,
        };
        let Some(Constant::String(name)) = program.constants.get(index as usize) else {
            return;
        };
        required = match name.to_scalar_string().ok().as_deref() {
            Some("RegExp") => required.with(ProviderCapability::RegExp),
            Some("BigInt") => required.with(ProviderCapability::BigInt),
            Some("JSON") => required.with(ProviderCapability::Json),
            Some("Temporal") => required.with(ProviderCapability::Temporal),
            Some("Intl") => required.with(ProviderCapability::Intl),
            // Two `String.prototype` methods answer from the Intl engine's tables (collation and
            // normalization are ICU4X data), so a program that calls either demands it by name.
            Some("localeCompare" | "normalize" | "toLocaleLowerCase" | "toLocaleUpperCase") => {
                required.with(ProviderCapability::Intl)
            }
            Some(
                "parallelMap" | "parallelFilter" | "parallelFlatMap" | "parallelReduce"
                | "parallelSort",
            ) => required.with(ProviderCapability::Parallel),
            _ => required,
        };
    };
    for instruction in retained_module_instructions(module, demanded) {
        inspect(instruction);
    }
    required
}

/// Provider engines reachable from the retained executable graph.
///
/// This is a liveness result, not a packaging choice. It reports individual capabilities even
/// while releases also ship common and full fallback archives; the provider resolver chooses the
/// smallest available variant whose declared set covers this one. Open-world code requests full.
pub fn retained_provider_capabilities(
    graph: &ModuleGraph,
    demand: &LiveDemand,
    members: &DemandedMembers,
) -> kiln_native_provider_contract::ProviderCapabilities {
    use kiln_module_resolver::NodeBuiltin;
    use kiln_native_provider_contract::{ProviderCapabilities, ProviderCapability};

    let mut required = ProviderCapabilities::REALM;
    for (name, capability) in [
        ("RegExp", ProviderCapability::RegExp),
        ("BigInt", ProviderCapability::BigInt),
        ("JSON", ProviderCapability::Json),
        ("Temporal", ProviderCapability::Temporal),
        ("Intl", ProviderCapability::Intl),
    ] {
        if members.globals.iter().any(|global| global == name) {
            required = required.with(capability);
        }
    }
    if [
        "parallelMap",
        "parallelFilter",
        "parallelFlatMap",
        "parallelReduce",
        "parallelSort",
    ]
    .iter()
    .any(|name| members.names.contains(*name))
    {
        required = required.with(ProviderCapability::Parallel);
    }
    if members.names.contains("localeCompare")
        || members.names.contains("normalize")
        || members.names.contains("toLocaleLowerCase")
        || members.names.contains("toLocaleUpperCase")
    {
        required = required.with(ProviderCapability::Intl);
    }

    let specifier_capability = |specifier: &str| match NodeBuiltin::parse(specifier) {
        Some(NodeBuiltin::Http | NodeBuiltin::Https | NodeBuiltin::Http2) => {
            Some(ProviderCapability::Http)
        }
        Some(NodeBuiltin::Ffi) => Some(ProviderCapability::Ffi),
        Some(NodeBuiltin::Gui) => Some(ProviderCapability::Gui),
        _ => None,
    };

    for node in graph.modules() {
        if !demand.live.contains(&node.path) {
            continue;
        }
        if members.has_open_world
            || matches!(node.source, ModuleSource::CommonJs)
            || !node.program.link_facts.dynamic_imports.is_empty()
            || node.program.program.has_dynamic_import()
        {
            return ProviderCapabilities::FULL;
        }
        if matches!(node.source, ModuleSource::Json) {
            required = required.with(ProviderCapability::Json);
        }
        required = required.union(program_provider_capabilities(
            &node.program,
            demand.slots.get(&node.path),
        ));
        if let ModuleSource::NodeBuiltin(builtin) = &node.source {
            required = match builtin {
                NodeBuiltin::Http | NodeBuiltin::Https | NodeBuiltin::Http2 => {
                    required.with(ProviderCapability::Http)
                }
                NodeBuiltin::Ffi => required.with(ProviderCapability::Ffi),
                NodeBuiltin::Gui => required.with(ProviderCapability::Gui),
                _ => required,
            };
        }
        for specifier in node
            .program
            .imports
            .iter()
            .map(|import| import.source.as_str())
            .chain(
                node.dependencies
                    .iter()
                    .map(|dependency| dependency.specifier.as_str()),
            )
        {
            if let Some(capability) = specifier_capability(specifier) {
                required = required.with(capability);
            }
        }
    }
    required
}

/// Per-module retained-size attribution for an executable graph.
///
/// Counts retained IR instructions and functions under the same closed-world
/// selection as emission, without emitting or compiling anything. The demand
/// applied per module mirrors emission exactly:
/// `Source`/`CommonJs` modules keep their demanded export slots, every other
/// module is retained whole. These neutral counts are not generated-C bytes.
/// Out-of-band: failures here must never fail a build, so callers skip the
/// section on error rather than propagating it.
pub fn executable_module_sizes(
    graph: &ModuleGraph,
    options: &SelectionOptions,
) -> Result<Vec<ModuleSizeFacts>, ModuleGraphError> {
    executable_module_sizes_from_demand(graph, &executable_demand(graph, options)?)
}

/// [`executable_module_sizes`] over an already-computed [`executable_demand`],
/// so callers needing both sizes and demand run the selection once.
pub fn executable_module_sizes_from_demand(
    graph: &ModuleGraph,
    demand: &LiveDemand,
) -> Result<Vec<ModuleSizeFacts>, ModuleGraphError> {
    let mut sizes = Vec::with_capacity(graph.module_count());
    for node in graph.modules() {
        let (total_instructions, total_functions) =
            kiln_graph::retained_module_size(&node.program, None);
        let is_live = demand.live.contains(&node.path);
        let (retained_instructions, retained_functions) = if !is_live {
            (0, 0)
        } else if matches!(
            node.source,
            kiln_module_graph::ModuleSource::Source | kiln_module_graph::ModuleSource::CommonJs
        ) {
            kiln_graph::retained_module_size(&node.program, demand.slots.get(&node.path))
        } else {
            (total_instructions, total_functions)
        };
        sizes.push(ModuleSizeFacts {
            path: node.path.display().to_string(),
            live: is_live,
            total_instructions,
            total_functions,
            retained_instructions,
            retained_functions,
        });
    }
    Ok(sizes)
}
