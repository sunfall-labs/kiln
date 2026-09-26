use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write;
use std::path::PathBuf;

use kiln_ir::{Constant, Instruction};
use kiln_module_graph::{ModuleGraph, ModuleSource};

/// Static member-name and builtin demand read off retained IR.
///
/// `names` holds every member name in a retained static position (`CallMethod`,
/// `GetProperty`, setters): the exact set a demand-driven method router would
/// serve. `builtins` holds retained `LoadBuiltin` variants by debug name.
/// Computed access (`x[k]`) and open-world needs (direct eval, dynamic import)
/// name nothing and are reported as flags: either forces the router to keep
/// its full table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DemandedMembers {
    pub names: BTreeSet<String>,
    pub builtins: BTreeSet<String>,
    pub globals: BTreeSet<String>,
    pub has_computed_access: bool,
    pub has_open_world: bool,
}

/// Read the demand in retained code: for every live module, scan its top-level
/// body and every function the closed-world analysis retains, mirroring the
/// demand emission applies (`Source`/`CommonJs` modules keep demanded slots,
/// every other module is retained whole; dropped modules are skipped).
pub fn analyze_demanded_members(
    graph: &ModuleGraph,
    live: &BTreeSet<PathBuf>,
    demands: &BTreeMap<PathBuf, BTreeSet<u32>>,
) -> DemandedMembers {
    let mut demanded = DemandedMembers::default();
    for node in graph.modules() {
        if !live.contains(&node.path) {
            continue;
        }
        let module = &node.program;
        let program = &module.program;
        demanded.has_open_world |= program.has_direct_eval() || program.has_dynamic_import();
        let is_closed_world = matches!(node.source, ModuleSource::Source | ModuleSource::CommonJs);
        // Retained function bodies, by index into `program.functions`. Top-level
        // code is always retained: it runs when the module evaluates.
        let reachable: Option<HashSet<usize>> = is_closed_world.then(|| {
            crate::closed_world_function_liveness(
                program,
                &demands.get(&node.path).cloned().unwrap_or_default(),
                module.link_facts.direct_eval,
                &module.link_facts.removable_declarations,
            )
            .0
        });
        let name_at = |index: u32| match program.constants.get(index as usize) {
            Some(Constant::String(name)) => name.to_scalar_string().ok(),
            _ => None,
        };
        let mut scan = |code: &[Instruction]| {
            for instruction in code {
                match instruction {
                    Instruction::LoadBuiltin(builtin) => {
                        demanded.builtins.insert(format!("{builtin:?}"));
                    }
                    Instruction::LoadGlobal(name)
                    | Instruction::RequireGlobalBinding(name)
                    | Instruction::LoadGlobalBindingOrUndefined(name)
                    | Instruction::ThrowReference(name) => {
                        if let Some(name) = name_at(*name) {
                            demanded.globals.insert(name);
                        }
                    }
                    Instruction::CallMethod { name, .. } => {
                        if let Some(name) = name_at(*name) {
                            demanded.names.insert(name);
                        }
                    }
                    Instruction::GetProperty(index) | Instruction::SetProperty(index) => {
                        if let Some(name) = name_at(*index) {
                            demanded.names.insert(name);
                        }
                    }
                    Instruction::SetPropertyStrict { name, .. } => {
                        if let Some(name) = name_at(*name) {
                            demanded.names.insert(name);
                        }
                    }
                    Instruction::GetComputed
                    | Instruction::CallComputed { .. }
                    | Instruction::SetComputed
                    | Instruction::SetComputedStrict { .. } => {
                        demanded.has_computed_access = true;
                    }
                    _ => {}
                }
            }
        };
        scan(&program.code);
        for (index, function) in program.functions.iter().enumerate() {
            if reachable.as_ref().is_none_or(|set| set.contains(&index)) {
                scan(&function.code);
            }
        }
    }
    demanded
}

/// Format member-name demand as a `kiln build --explain` section.
pub fn format_demanded_members(demanded: &DemandedMembers) -> String {
    let mut out = String::new();
    out.push_str("\n=== Demanded Members (retained IR) ===\n");
    let _ = writeln!(
        out,
        "Summary: {} distinct names, {} builtins, {} globals, computed access: {}, open world: {}",
        demanded.names.len(),
        demanded.builtins.len(),
        demanded.globals.len(),
        demanded.has_computed_access,
        demanded.has_open_world
    );
    if !demanded.builtins.is_empty() {
        out.push_str("Builtins:\n");
        for builtin in &demanded.builtins {
            let _ = writeln!(out, "  {builtin}");
        }
    }
    if !demanded.globals.is_empty() {
        out.push_str("Globals:\n");
        for global in &demanded.globals {
            let _ = writeln!(out, "  {global}");
        }
    }
    if !demanded.names.is_empty() {
        out.push_str("Names:\n");
        for name in &demanded.names {
            let _ = writeln!(out, "  {name}");
        }
    }
    out
}

/// Method dispatch group bits for the demand-driven prelude router.
///
/// Values mirror the `KILN_C99_GROUP_*` defines in `prelude.rs` exactly; the
/// `method_registry_covers_every_group` test fails if the C table ever uses a
/// bit this module does not classify, forcing the new group into either the
/// always-core set or the prunable set.
pub mod method_groups {
    pub const DIRECT: u32 = 1;
    pub const ARRAY_LOOKUP: u32 = 2;
    pub const STRING_DISPATCH: u32 = 4;
    pub const GENERIC: u32 = 8;
    pub const TEMPORAL: u32 = 16;
    pub const TEMPORAL_FROM: u32 = 32;
    pub const TEMPORAL_COMPARE: u32 = 64;
    pub const TEMPORAL_NOW_READ: u32 = 128;
    pub const TEMPORAL_OP: u32 = 256;
    pub const DATE_SETTER: u32 = 512;
    pub const NODE_EVENT: u32 = 1024;
    pub const NODE_NET: u32 = 2048;
    pub const NODE_SERVER: u32 = 4096;
    pub const NODE_HTTP: u32 = 8192;
    pub const NODE_RESPONSE: u32 = 16384;
    pub const BUFFER: u32 = 32768;
    pub const NODE_UTIL: u32 = 65536;
    pub const NODE_DIAGNOSTICS: u32 = 131072;
    pub const NODE_STREAM: u32 = 262144;
    pub const NODE_PATH: u32 = 524288;
    pub const MATH_NUMBER: u32 = 1048576;
    /// `URL` and `URLSearchParams`: the `url` crate through the provider, and the class in the
    /// runtime bootstrap. Close to a megabyte of `idna` tables, so a program that names
    /// neither -- and imports nothing that does -- links none of it (ADR 0063 rule 4).
    pub const URL: u32 = 2097152;
}

/// Dispatch machinery required by every program regardless of the globals it reaches.
pub const CORE_METHOD_GROUPS: u32 = method_groups::DIRECT
    | method_groups::ARRAY_LOOKUP
    | method_groups::STRING_DISPATCH
    | method_groups::GENERIC;

/// Groups the router may drop when no demand implicates them.
pub const PRUNABLE_METHOD_GROUPS: u32 = method_groups::URL
    | method_groups::NODE_EVENT
    | method_groups::NODE_NET
    | method_groups::NODE_SERVER
    | method_groups::NODE_HTTP
    | method_groups::NODE_RESPONSE
    | method_groups::BUFFER
    | method_groups::NODE_UTIL
    | method_groups::NODE_DIAGNOSTICS
    | method_groups::NODE_STREAM
    | method_groups::NODE_PATH
    | method_groups::TEMPORAL
    | method_groups::TEMPORAL_FROM
    | method_groups::TEMPORAL_COMPARE
    | method_groups::TEMPORAL_NOW_READ
    | method_groups::TEMPORAL_OP
    | method_groups::DATE_SETTER
    | method_groups::MATH_NUMBER;

/// Human names for a group mask, in canonical dispatch order.
pub fn method_group_names(mask: u32) -> Vec<&'static str> {
    const ALL: [(u32, &str); 21] = [
        (method_groups::DIRECT, "Direct"),
        (method_groups::ARRAY_LOOKUP, "ArrayLookup"),
        (method_groups::STRING_DISPATCH, "StringDispatch"),
        (method_groups::GENERIC, "Generic"),
        (method_groups::TEMPORAL, "Temporal"),
        (method_groups::TEMPORAL_FROM, "TemporalFrom"),
        (method_groups::TEMPORAL_COMPARE, "TemporalCompare"),
        (method_groups::TEMPORAL_NOW_READ, "TemporalNowRead"),
        (method_groups::TEMPORAL_OP, "TemporalOp"),
        (method_groups::DATE_SETTER, "DateSetter"),
        (method_groups::NODE_EVENT, "NodeEvent"),
        (method_groups::NODE_NET, "NodeNet"),
        (method_groups::NODE_SERVER, "NodeServer"),
        (method_groups::NODE_HTTP, "NodeHttp"),
        (method_groups::NODE_RESPONSE, "NodeResponse"),
        (method_groups::BUFFER, "Buffer"),
        (method_groups::NODE_UTIL, "NodeUtil"),
        (method_groups::NODE_DIAGNOSTICS, "NodeDiagnostics"),
        (method_groups::NODE_STREAM, "NodeStream"),
        (method_groups::NODE_PATH, "NodePath"),
        (method_groups::MATH_NUMBER, "MathNumber"),
    ];
    ALL.into_iter()
        .filter(|(bit, _)| mask & bit != 0)
        .map(|(_, name)| name)
        .collect()
}

/// Format a retained dispatch-group mask as a `kiln build --explain` section.
pub fn format_retained_groups(mask: u32) -> String {
    let mut out = String::new();
    out.push_str("Retained dispatch groups:\n");
    for name in method_group_names(mask) {
        let _ = writeln!(out, "  {name}");
    }
    out
}
