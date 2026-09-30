//! Facts analysis and explanation over programs and linked module graphs.
//!
//! Provides whole-program fixpoint information including:
//! - Closed-world vs open-world verdict
//! - Immutable module bindings across the graph
//! - Per-function summaries with inferred parameter and return types
//! - Allocation, call, and property access sites

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write;
use std::path::Path;

use kiln_ir::{Constant, FunctionProto, Instruction, Program};
use kiln_module_graph::{ClosedWorldVerdict, ModuleGraph};
use serde::{Deserialize, Serialize};

use crate::graph::{FunctionGraph, GraphOp, build_function_graph};
use crate::summary::{TypeGuess, summarize_graph};

/// Deterministic description of an allocation, call, or property access site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteFact {
    pub id: u32,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<String>,
}

/// Inferred facts for a compiled function or top-level body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionFacts {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub parameter_types: Vec<TypeGuess>,
    pub return_type: TypeGuess,
    pub pure: bool,
    pub can_throw: bool,
    pub reads_arguments: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub returned_sites: Vec<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub sites: Vec<SiteFact>,
}

/// Inferred facts for a single module within a linked graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleFactsReport {
    pub path: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub immutable_bindings: Vec<String>,
    pub functions: Vec<FunctionFacts>,
}

/// Inferred facts for an HTTP request handler within a closed-world program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandlerFacts {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub function_name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub request_members_read: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub request_members_omitted: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub response_members_read: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub response_members_omitted: Vec<String>,
    pub url_specialized: bool,
    pub instructions_per_request: usize,
}

/// Whole-program fixpoint facts over a linked graph or standalone program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramFacts {
    pub world: ClosedWorldVerdict,
    pub modules: Vec<ModuleFactsReport>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub handlers: Vec<HandlerFacts>,
}

/// Retained instruction/function counts for one module in a linked executable.
///
/// `total_*` counts every instruction/function in the module; `retained_*`
/// counts what closed-world liveness keeps under the build's export demand.
/// These IR counts are not generated-C byte sizes.
/// A module the executable drops entirely is reported with `live: false` and
/// zero retained counts. Paths are display strings, like [`ModuleFactsReport`].
pub use kiln_liveness::ModuleSizeFacts;

/// Format per-module retained sizes as a `kiln build --explain` section.
///
/// Sorted by retained instructions descending (then path) so the largest
/// contributors read first regardless of the caller's order.
pub fn format_module_sizes(sizes: &[ModuleSizeFacts]) -> String {
    let mut ordered: Vec<&ModuleSizeFacts> = sizes.iter().collect();
    ordered.sort_by(|a, b| {
        b.retained_instructions
            .cmp(&a.retained_instructions)
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut out = String::new();
    out.push_str("\n=== Module Sizes (retained / total IR) ===\n");
    let live = ordered.iter().filter(|size| size.live).count();
    let dropped = ordered.len() - live;
    let (retained_instructions, total_instructions, retained_functions, total_functions) =
        ordered.iter().fold((0, 0, 0, 0), |sums, size| {
            (
                sums.0 + size.retained_instructions,
                sums.1 + size.total_instructions,
                sums.2 + size.retained_functions,
                sums.3 + size.total_functions,
            )
        });
    let _ = writeln!(
        out,
        "Summary: {live} live modules, {dropped} dropped, {retained_instructions}/{total_instructions} instructions, {retained_functions}/{total_functions} functions retained"
    );
    for size in ordered {
        if size.live {
            let _ = writeln!(
                out,
                "  {:>8} instr {:>4} fns  {}",
                format!("{}/{}", size.retained_instructions, size.total_instructions),
                format!("{}/{}", size.retained_functions, size.total_functions),
                size.path
            );
        } else {
            let _ = writeln!(
                out,
                "  dropped ({}/{} instr, {}/{} fns)  {}",
                size.retained_instructions,
                size.total_instructions,
                size.retained_functions,
                size.total_functions,
                size.path
            );
        }
    }
    out
}

pub use kiln_graph::{
    CORE_METHOD_GROUPS, DemandedMembers, PRUNABLE_METHOD_GROUPS, analyze_demanded_members,
    format_demanded_members, format_retained_groups, method_group_names, method_groups,
};

impl ProgramFacts {
    /// Format these facts as human-readable explanation output for `kiln build --explain`.
    pub fn format_explain(&self) -> String {
        let mut out = String::new();
        out.push_str("=== Whole-Program Fixpoint Facts ===\n");

        match &self.world {
            ClosedWorldVerdict::Closed => {
                out.push_str("World: closed\n");
            }
            ClosedWorldVerdict::Open { reasons } => {
                out.push_str("World: open\n");
                out.push_str("  Reasons:\n");
                for reason in reasons {
                    let _ = writeln!(out, "    - {reason}");
                }
            }
        }

        out.push('\n');
        out.push_str("Immutable Bindings:\n");
        let mut any_bindings = false;
        for module in &self.modules {
            if !module.immutable_bindings.is_empty() {
                any_bindings = true;
                let _ = writeln!(out, "  {}:", module.path);
                for binding in &module.immutable_bindings {
                    let _ = writeln!(out, "    - {binding}");
                }
            }
        }
        if !any_bindings {
            out.push_str("  (none)\n");
        }

        out.push('\n');
        out.push_str("Functions & Sites:\n");
        for module in &self.modules {
            let _ = writeln!(out, "  Module {}:", module.path);
            for func in &module.functions {
                let name = func.name.as_deref().unwrap_or("<anonymous>");
                let param_str: Vec<String> = func
                    .parameter_types
                    .iter()
                    .map(|t| format!("{t:?}"))
                    .collect();
                let _ = writeln!(
                    out,
                    "    Function {} (params: [{}], return: {:?}, pure: {}, can_throw: {}, reads_arguments: {})",
                    name,
                    param_str.join(", "),
                    func.return_type,
                    func.pure,
                    func.can_throw,
                    func.reads_arguments
                );
                if func.sites.is_empty() {
                    out.push_str("      Sites: (none)\n");
                } else {
                    out.push_str("      Sites:\n");
                    for site in &func.sites {
                        if let Some(detail) = &site.detail {
                            let _ = writeln!(
                                out,
                                "        - site {}: {} ({detail})",
                                site.id, site.kind
                            );
                        } else {
                            let _ = writeln!(out, "        - site {}: {}", site.id, site.kind);
                        }
                    }
                }
            }
        }

        if !self.handlers.is_empty() {
            out.push('\n');
            out.push_str("Handler Specialization:\n");
            for handler in &self.handlers {
                let name = handler.function_name.as_deref().unwrap_or("<anonymous>");
                let _ = writeln!(out, "  Handler {name}:");
                let _ = writeln!(
                    out,
                    "    Request members read: [{}]",
                    handler.request_members_read.join(", ")
                );
                let _ = writeln!(
                    out,
                    "    Request members omitted: [{}]",
                    handler.request_members_omitted.join(", ")
                );
                let _ = writeln!(
                    out,
                    "    Response members read: [{}]",
                    handler.response_members_read.join(", ")
                );
                let _ = writeln!(
                    out,
                    "    Response members omitted: [{}]",
                    handler.response_members_omitted.join(", ")
                );
                let _ = writeln!(out, "    URL specialized: {}", handler.url_specialized);
                let _ = writeln!(
                    out,
                    "    JS instructions per request: {}",
                    handler.instructions_per_request
                );
            }
        }

        out
    }

    /// Total number of sites in this program.
    pub fn site_count(&self) -> usize {
        self.modules
            .iter()
            .flat_map(|m| &m.functions)
            .map(|f| f.sites.len())
            .sum()
    }

    /// Total number of functions in this program.
    pub fn function_count(&self) -> usize {
        self.modules.iter().map(|m| m.functions.len()).sum()
    }

    /// Total number of immutable bindings in this program.
    pub fn immutable_binding_count(&self) -> usize {
        self.modules
            .iter()
            .map(|m| m.immutable_bindings.len())
            .sum()
    }

    /// Normalize all module paths relative to the given repository root.
    /// Also sorts modules and immutable bindings deterministically.
    pub fn normalize_paths(&mut self, repo_root: &Path) {
        let root_str = repo_root.to_str().unwrap_or("");
        if let ClosedWorldVerdict::Open { ref mut reasons } = self.world {
            for reason in reasons {
                if !root_str.is_empty() && reason.contains(root_str) {
                    *reason = reason
                        .replace(root_str, "")
                        .trim_start_matches('/')
                        .to_string();
                }
            }
        }
        for module in &mut self.modules {
            if let Ok(rel) = Path::new(&module.path).strip_prefix(repo_root) {
                module.path = rel.to_string_lossy().into_owned();
            } else if !root_str.is_empty()
                && let Some(stripped) = module.path.strip_prefix(root_str)
            {
                module.path = stripped.trim_start_matches('/').to_string();
            }
            module.immutable_bindings.sort();
        }
        self.modules.sort_by(|a, b| a.path.cmp(&b.path));
        // A verdict's reason names the module it was found in. It is recorded in a baseline
        // read on other machines, so it names it relative to the checkout, like the modules.
        if let ClosedWorldVerdict::Open { reasons } = &mut self.world
            && !root_str.is_empty()
        {
            let prefix = format!("{}/", root_str.trim_end_matches('/'));
            for reason in reasons.iter_mut() {
                *reason = reason.replace(&prefix, "");
            }
        }
    }

    /// Compare this ProgramFacts against an expected baseline ProgramFacts.
    /// Returns a list of discrepancies (empty if identical).
    pub fn compare(&self, expected: &ProgramFacts) -> Vec<String> {
        let mut diffs = Vec::new();

        if self.world != expected.world {
            diffs.push(format!(
                "world verdict changed: was {:?}, now {:?}",
                expected.world, self.world
            ));
        }

        if self.handlers != expected.handlers {
            diffs.push(format!(
                "handlers changed: was {:?}, now {:?}",
                expected.handlers, self.handlers
            ));
        }

        let self_mods: BTreeMap<&str, &ModuleFactsReport> =
            self.modules.iter().map(|m| (m.path.as_str(), m)).collect();
        let exp_mods: BTreeMap<&str, &ModuleFactsReport> = expected
            .modules
            .iter()
            .map(|m| (m.path.as_str(), m))
            .collect();

        for path in exp_mods.keys() {
            if !self_mods.contains_key(path) {
                diffs.push(format!("module missing in current run: {path}"));
            }
        }
        for path in self_mods.keys() {
            if !exp_mods.contains_key(path) {
                diffs.push(format!("new module in current run: {path}"));
            }
        }

        for (path, exp_mod) in &exp_mods {
            if let Some(act_mod) = self_mods.get(path) {
                if act_mod.immutable_bindings != exp_mod.immutable_bindings {
                    diffs.push(format!(
                        "module {path}: immutable bindings changed: was {:?}, now {:?}",
                        exp_mod.immutable_bindings, act_mod.immutable_bindings
                    ));
                }

                if act_mod.functions.len() != exp_mod.functions.len() {
                    diffs.push(format!(
                        "module {path}: function count changed: was {}, now {}",
                        exp_mod.functions.len(),
                        act_mod.functions.len()
                    ));
                }

                let max_len = act_mod.functions.len().max(exp_mod.functions.len());
                for i in 0..max_len {
                    match (exp_mod.functions.get(i), act_mod.functions.get(i)) {
                        (Some(exp_fn), Some(act_fn)) => {
                            let fn_name = exp_fn
                                .name
                                .as_deref()
                                .or(act_fn.name.as_deref())
                                .unwrap_or("<anon>");
                            if exp_fn.name != act_fn.name {
                                diffs.push(format!(
                                    "module {path} func #{i}: name changed: was {:?}, now {:?}",
                                    exp_fn.name, act_fn.name
                                ));
                            }
                            if exp_fn.parameter_types != act_fn.parameter_types {
                                diffs.push(format!(
                                    "module {path} func #{i} ({fn_name}): param types changed: was {:?}, now {:?}",
                                    exp_fn.parameter_types, act_fn.parameter_types
                                ));
                            }
                            if exp_fn.return_type != act_fn.return_type {
                                diffs.push(format!(
                                    "module {path} func #{i} ({fn_name}): return type changed: was {:?}, now {:?}",
                                    exp_fn.return_type, act_fn.return_type
                                ));
                            }
                            if exp_fn.pure != act_fn.pure {
                                diffs.push(format!(
                                    "module {path} func #{i} ({fn_name}): purity changed: was {}, now {}",
                                    exp_fn.pure, act_fn.pure
                                ));
                            }
                            if exp_fn.can_throw != act_fn.can_throw {
                                diffs.push(format!(
                                    "module {path} func #{i} ({fn_name}): can_throw changed: was {}, now {}",
                                    exp_fn.can_throw, act_fn.can_throw
                                ));
                            }
                            if exp_fn.reads_arguments != act_fn.reads_arguments {
                                diffs.push(format!(
                                    "module {path} func #{i} ({fn_name}): reads_arguments changed: was {}, now {}",
                                    exp_fn.reads_arguments, act_fn.reads_arguments
                                ));
                            }
                            if exp_fn.returned_sites != act_fn.returned_sites {
                                diffs.push(format!(
                                    "module {path} func #{i} ({fn_name}): returned_sites changed: was {:?}, now {:?}",
                                    exp_fn.returned_sites, act_fn.returned_sites
                                ));
                            }
                            if exp_fn.sites != act_fn.sites {
                                diffs.push(format!(
                                    "module {path} func #{i} ({fn_name}): sites changed: was {:?}, now {:?}",
                                    exp_fn.sites, act_fn.sites
                                ));
                            }
                        }
                        (Some(exp_fn), None) => {
                            let fn_name = exp_fn.name.as_deref().unwrap_or("<anon>");
                            diffs.push(format!("module {path}: missing function #{i} ({fn_name})"));
                        }
                        (None, Some(act_fn)) => {
                            let fn_name = act_fn.name.as_deref().unwrap_or("<anon>");
                            diffs.push(format!("module {path}: extra function #{i} ({fn_name})"));
                        }
                        (None, None) => unreachable!(),
                    }
                }
            }
        }

        diffs
    }
}

/// Top-level snapshot of whole-program fixpoint facts across benchmarks and thirdparty bundles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactsBaseline {
    pub schema: u32,
    #[serde(default)]
    pub benchmarks: BTreeMap<String, ProgramFacts>,
    #[serde(default)]
    pub bundles: BTreeMap<String, ProgramFacts>,
}

impl Default for FactsBaseline {
    fn default() -> Self {
        Self {
            schema: 1,
            benchmarks: BTreeMap::new(),
            bundles: BTreeMap::new(),
        }
    }
}

impl FactsBaseline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a baseline document from JSON string.
    pub fn from_json_str(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Serialize this baseline to formatted JSON string.
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self).map(|mut s| {
            s.push('\n');
            s
        })
    }

    /// Compare this baseline against an expected baseline chunk by chunk.
    /// Returns a map of `chunk_key -> Vec<discrepancies>`.
    pub fn compare(&self, expected: &FactsBaseline) -> BTreeMap<String, Vec<String>> {
        let mut discrepancies = BTreeMap::new();

        for (name, exp_facts) in &expected.benchmarks {
            let key = format!("benchmark:{name}");
            if let Some(act_facts) = self.benchmarks.get(name) {
                let diffs = act_facts.compare(exp_facts);
                if !diffs.is_empty() {
                    discrepancies.insert(key, diffs);
                }
            } else {
                discrepancies.insert(key, vec!["target missing in current run".to_string()]);
            }
        }
        for name in self.benchmarks.keys() {
            let key = format!("benchmark:{name}");
            if !expected.benchmarks.contains_key(name) {
                discrepancies.insert(key, vec!["new target not present in baseline".to_string()]);
            }
        }

        for (name, exp_facts) in &expected.bundles {
            let key = format!("bundle:{name}");
            if let Some(act_facts) = self.bundles.get(name) {
                let diffs = act_facts.compare(exp_facts);
                if !diffs.is_empty() {
                    discrepancies.insert(key, diffs);
                }
            } else {
                discrepancies.insert(key, vec!["target missing in current run".to_string()]);
            }
        }
        for name in self.bundles.keys() {
            let key = format!("bundle:{name}");
            if !expected.bundles.contains_key(name) {
                discrepancies.insert(key, vec!["new target not present in baseline".to_string()]);
            }
        }

        discrepancies
    }
}

/// Analyze a complete linked `ModuleGraph` and extract whole-program facts.
pub fn analyze_graph(graph: &ModuleGraph) -> ProgramFacts {
    let world = graph.closed_world_verdict();
    let mut modules = Vec::with_capacity(graph.module_count());

    for node in graph.modules() {
        let path = node.path.display().to_string();
        let mut immutable_bindings = node.program.link_facts.immutable_bindings.clone();
        immutable_bindings.sort();
        let functions = analyze_program_functions(&node.program.program);
        modules.push(ModuleFactsReport {
            path,
            immutable_bindings,
            functions,
        });
    }

    modules.sort_by(|a, b| a.path.cmp(&b.path));
    let handlers = extract_handlers_from_graph(graph);

    ProgramFacts {
        world,
        modules,
        handlers,
    }
}

/// Analyze a standalone `Program` and extract facts.
pub fn analyze_program(program: &Program, path: Option<&str>) -> ProgramFacts {
    let has_eval = program.has_direct_eval();
    let has_dynamic_import = program.has_dynamic_import();
    let world = if has_eval || has_dynamic_import {
        let mut reasons = Vec::new();
        if has_eval {
            reasons.push("program contains direct eval".to_string());
        }
        if has_dynamic_import {
            reasons.push("program contains dynamic import".to_string());
        }
        ClosedWorldVerdict::Open { reasons }
    } else {
        ClosedWorldVerdict::Closed
    };

    let functions = analyze_program_functions(program);
    let module = ModuleFactsReport {
        path: path.unwrap_or("<main>").to_string(),
        immutable_bindings: Vec::new(),
        functions,
    };
    let handlers = extract_handlers_from_program(program);

    ProgramFacts {
        world,
        modules: vec![module],
        handlers,
    }
}

pub const KNOWN_REQUEST_MEMBERS: &[&str] = &[
    "aborted",
    "complete",
    "connection",
    "destroyed",
    "headers",
    "httpVersion",
    "httpVersionMajor",
    "httpVersionMinor",
    "method",
    "on",
    "once",
    "rawHeaders",
    "socket",
    "statusCode",
    "statusMessage",
    "url",
];

pub const KNOWN_RESPONSE_MEMBERS: &[&str] = &[
    "connection",
    "destroyed",
    "end",
    "finished",
    "getHeader",
    "hasHeader",
    "headersSent",
    "removeHeader",
    "setHeader",
    "socket",
    "statusCode",
    "statusMessage",
    "write",
    "writeHead",
    "writableEnded",
];

pub const DISTINCTIVE_REQUEST_MEMBERS: &[&str] = &["url", "headers", "rawHeaders", "httpVersion"];

pub const DISTINCTIVE_RESPONSE_MEMBERS: &[&str] = &[
    "end",
    "write",
    "writeHead",
    "setHeader",
    "getHeader",
    "hasHeader",
    "removeHeader",
    "headersSent",
    "writableEnded",
];

pub fn extract_handlers_from_graph(graph: &ModuleGraph) -> Vec<HandlerFacts> {
    let mut handlers = Vec::new();
    for node in graph.modules() {
        handlers.extend(extract_handlers_from_program(&node.program.program));
    }
    handlers
}

pub fn extract_handlers_from_program(program: &Program) -> Vec<HandlerFacts> {
    let mut handlers = Vec::new();
    for proto in &program.functions {
        if proto.parameter_count < 2 {
            continue;
        }

        let mut req_members_read = BTreeSet::new();
        let mut res_members_read = BTreeSet::new();
        let mut url_specialized = false;

        // Try analyzing via SSA graph first
        if let Ok(graph) = build_function_graph(proto, program) {
            let mut req_values = HashSet::new();
            let mut res_values = HashSet::new();
            if let Some(&vid) = graph.initial_locals.first() {
                req_values.insert(vid);
            }
            if let Some(&vid) = graph.initial_locals.get(1) {
                res_values.insert(vid);
            }

            // Propagate aliases through loads and phis
            let mut changed = true;
            while changed {
                changed = false;
                for block in &graph.blocks {
                    for (vid, op) in &block.operations {
                        match op {
                            GraphOp::LoadLocal(0) | GraphOp::LoadCell(0) => {
                                if req_values.insert(*vid) {
                                    changed = true;
                                }
                            }
                            GraphOp::LoadLocal(1) | GraphOp::LoadCell(1) => {
                                if res_values.insert(*vid) {
                                    changed = true;
                                }
                            }
                            GraphOp::Phi { inputs } => {
                                if inputs.iter().any(|(_, v)| req_values.contains(v))
                                    && req_values.insert(*vid)
                                {
                                    changed = true;
                                }
                                if inputs.iter().any(|(_, v)| res_values.contains(v))
                                    && res_values.insert(*vid)
                                {
                                    changed = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            let mut req_url_values = HashSet::new();
            let mut url_values = HashSet::new();
            let mut search_params_values = HashSet::new();

            for block in &graph.blocks {
                for (vid, op) in &block.operations {
                    match op {
                        GraphOp::GetProperty { object, name, .. } => {
                            let prop_name = program.constants.get(*name as usize).and_then(|c| {
                                if let Constant::String(s) = c {
                                    s.to_scalar_string().ok()
                                } else {
                                    None
                                }
                            });
                            if let Some(prop_name) = prop_name {
                                if req_values.contains(object) {
                                    req_members_read.insert(prop_name.to_string());
                                    if prop_name == "url" {
                                        req_url_values.insert(*vid);
                                    }
                                }
                                if res_values.contains(object) {
                                    res_members_read.insert(prop_name.to_string());
                                }
                                if url_values.contains(object) {
                                    if prop_name == "pathname" {
                                        url_specialized = true;
                                    } else if prop_name == "searchParams" {
                                        search_params_values.insert(*vid);
                                    }
                                }
                            }
                        }
                        GraphOp::CallMethod { object, name, .. } => {
                            let prop_name = program.constants.get(*name as usize).and_then(|c| {
                                if let Constant::String(s) = c {
                                    s.to_scalar_string().ok()
                                } else {
                                    None
                                }
                            });
                            if let Some(prop_name) = prop_name {
                                if req_values.contains(object) {
                                    req_members_read.insert(prop_name.to_string());
                                }
                                if res_values.contains(object) {
                                    res_members_read.insert(prop_name.to_string());
                                }
                                if url_values.contains(object) && prop_name == "pathname" {
                                    url_specialized = true;
                                }
                                if search_params_values.contains(object) && prop_name == "get" {
                                    url_specialized = true;
                                }
                            }
                        }
                        GraphOp::SetProperty { object, name, .. }
                        | GraphOp::SetPropertyStrict { object, name, .. }
                        | GraphOp::DefineOwnProperty { object, name, .. } => {
                            let prop_name = program.constants.get(*name as usize).and_then(|c| {
                                if let Constant::String(s) = c {
                                    s.to_scalar_string().ok()
                                } else {
                                    None
                                }
                            });
                            if let Some(prop_name) = prop_name
                                && res_values.contains(object)
                            {
                                res_members_read.insert(prop_name.to_string());
                            }
                        }
                        GraphOp::New { args, .. }
                            if !args.is_empty() && req_url_values.contains(&args[0]) =>
                        {
                            url_values.insert(*vid);
                        }
                        _ => {}
                    }
                }
            }
        }

        // Also check bytecode directly to ensure complete coverage (e.g. if graph was not built
        // or for property accesses like pathname/searchParams on aliased URL values)
        let mut eval_stack: Vec<Option<u32>> = Vec::new();
        let mut has_pathname = false;
        let mut has_search_params = false;

        for inst in &proto.code {
            match inst {
                Instruction::LoadLocal(slot) | Instruction::LoadCell(slot) => {
                    eval_stack.push(Some(*slot));
                }
                Instruction::GetProperty(name) => {
                    let obj = eval_stack.pop().flatten();
                    if let Some(Constant::String(prop)) = program.constants.get(*name as usize) {
                        if obj == Some(0) {
                            req_members_read.insert(prop.to_scalar_string().unwrap_or_default());
                        } else if obj == Some(1) {
                            res_members_read.insert(prop.to_scalar_string().unwrap_or_default());
                        }
                        if prop == "pathname" {
                            has_pathname = true;
                        } else if prop == "searchParams" {
                            has_search_params = true;
                        }
                    }
                    eval_stack.push(None);
                }
                Instruction::CallMethod { name, argc } => {
                    let argc = *argc as usize;
                    let receiver = if eval_stack.len() > argc {
                        eval_stack[eval_stack.len() - 1 - argc]
                    } else {
                        None
                    };
                    if let Some(Constant::String(prop)) = program.constants.get(*name as usize) {
                        if receiver == Some(0) {
                            req_members_read.insert(prop.to_scalar_string().unwrap_or_default());
                        } else if receiver == Some(1) {
                            res_members_read.insert(prop.to_scalar_string().unwrap_or_default());
                        }
                    }
                    if eval_stack.len() > argc {
                        eval_stack.truncate(eval_stack.len() - 1 - argc);
                    } else {
                        eval_stack.clear();
                    }
                    eval_stack.push(None);
                }
                Instruction::SetProperty(name)
                | Instruction::SetPropertyStrict { name, .. }
                | Instruction::DefineOwnProperty(name) => {
                    eval_stack.pop();
                    let obj = eval_stack.pop().flatten();
                    if obj == Some(1)
                        && let Some(Constant::String(prop)) = program.constants.get(*name as usize)
                    {
                        res_members_read.insert(prop.to_scalar_string().unwrap_or_default());
                    }
                    eval_stack.push(None);
                }
                Instruction::Pop => {
                    eval_stack.pop();
                }
                Instruction::Dup => {
                    let top = eval_stack.last().copied().flatten();
                    eval_stack.push(top);
                }
                Instruction::Call { argc } | Instruction::New { argc } => {
                    let total = 1 + *argc as usize;
                    if eval_stack.len() >= total {
                        eval_stack.truncate(eval_stack.len() - total);
                    } else {
                        eval_stack.clear();
                    }
                    eval_stack.push(None);
                }
                _ => {
                    eval_stack.push(None);
                }
            }
        }

        if req_members_read.contains("url") && (has_pathname || has_search_params) {
            url_specialized = true;
        }

        let has_res_member = res_members_read
            .iter()
            .any(|m| KNOWN_RESPONSE_MEMBERS.contains(&m.as_str()));
        let param0_has_res_only = req_members_read.iter().any(|m| {
            KNOWN_RESPONSE_MEMBERS.contains(&m.as_str())
                && !KNOWN_REQUEST_MEMBERS.contains(&m.as_str())
        });
        let has_distinctive_res = res_members_read
            .iter()
            .any(|m| DISTINCTIVE_RESPONSE_MEMBERS.contains(&m.as_str()));
        let has_distinctive_req = req_members_read
            .iter()
            .any(|m| DISTINCTIVE_REQUEST_MEMBERS.contains(&m.as_str()));

        // It is an HTTP handler if param 1 uses distinctive response members, or param 0
        // uses distinctive request members along with response members, and param 0 does
        // not exclusively read response members.
        let is_handler = (has_distinctive_res || (has_distinctive_req && has_res_member))
            && !param0_has_res_only;

        if is_handler {
            let request_members_omitted = KNOWN_REQUEST_MEMBERS
                .iter()
                .filter(|&&m| !req_members_read.contains(m))
                .map(|&m| m.to_string())
                .collect();
            let response_members_omitted = KNOWN_RESPONSE_MEMBERS
                .iter()
                .filter(|&&m| !res_members_read.contains(m))
                .map(|&m| m.to_string())
                .collect();

            handlers.push(HandlerFacts {
                function_name: proto.name.clone(),
                request_members_read: req_members_read.into_iter().collect(),
                request_members_omitted,
                response_members_read: res_members_read.into_iter().collect(),
                response_members_omitted,
                url_specialized,
                instructions_per_request: proto.code.len(),
            });
        }
    }

    handlers
}

fn analyze_program_functions(program: &Program) -> Vec<FunctionFacts> {
    let mut functions = Vec::new();

    // Analyze top-level code as a function
    if !program.code.is_empty() {
        let top_level_proto = FunctionProto {
            inferred_name: None,
            asynchronous: false,
            name: Some("<top-level>".to_string()),
            arrow: false,
            generator: false,
            async_generator: false,
            is_method: false,
            is_class: false,
            parameter_count: 0,
            mapped_arguments: false,
            declared_length: 0,
            local_count: program.local_count,
            prologue_end: 0,
            code: program.code.clone(),
            upvalues: Vec::new(),
            span: kiln_ir::Span::default(),
            type_annotations: None,
        };
        if let Ok(graph) = build_function_graph(&top_level_proto, program) {
            functions.push(extract_function_facts(&graph, program));
        }
    }

    // Analyze each declared function
    for proto in &program.functions {
        if let Ok(graph) = build_function_graph(proto, program) {
            functions.push(extract_function_facts(&graph, program));
        }
    }

    functions
}

fn extract_function_facts(graph: &FunctionGraph, program: &Program) -> FunctionFacts {
    let summary = summarize_graph(graph);
    let mut sites = Vec::new();

    for block in &graph.blocks {
        for (_value_id, op) in &block.operations {
            match op {
                GraphOp::GetProperty { name, site, .. } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "GetProperty".to_string(),
                        detail: Some(format_constant_name(program, *name)),
                    });
                }
                GraphOp::SetProperty { name, site, .. } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "SetProperty".to_string(),
                        detail: Some(format_constant_name(program, *name)),
                    });
                }
                GraphOp::SetPropertyStrict { name, site, .. } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "SetPropertyStrict".to_string(),
                        detail: Some(format_constant_name(program, *name)),
                    });
                }
                GraphOp::DefineOwnProperty { name, site, .. } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "DefineOwnProperty".to_string(),
                        detail: Some(format_constant_name(program, *name)),
                    });
                }
                GraphOp::GetComputed { site, .. } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "GetComputed".to_string(),
                        detail: None,
                    });
                }
                GraphOp::SetComputed { site, .. } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "SetComputed".to_string(),
                        detail: None,
                    });
                }
                GraphOp::SetComputedStrict { site, .. } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "SetComputedStrict".to_string(),
                        detail: None,
                    });
                }
                GraphOp::DefineOwnComputed { site, .. } => {
                    sites.push(SiteFact { id: site.0, kind: "DefineOwnComputed".to_string(), detail: None });
                }
                GraphOp::MakeFunction { index, site } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "MakeFunction".to_string(),
                        detail: Some(format!("proto #{index}")),
                    });
                }
                GraphOp::Call { site, .. } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "Call".to_string(),
                        detail: None,
                    });
                }
                GraphOp::CallMethod { name, site, .. } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "CallMethod".to_string(),
                        detail: Some(format_constant_name(program, *name)),
                    });
                }
                GraphOp::NewArray { site, .. } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "NewArray".to_string(),
                        detail: None,
                    });
                }
                _ => {}
            }
        }
    }

    sites.sort_by_key(|s| s.id);
    sites.dedup_by_key(|s| s.id);

    FunctionFacts {
        name: graph.name.clone(),
        parameter_types: summary.parameter_types,
        return_type: summary.return_type,
        pure: summary.pure_or_unknown,
        can_throw: summary.can_throw,
        reads_arguments: summary.reads_arguments,
        returned_sites: summary.returned_sites,
        sites,
    }
}

fn format_constant_name(program: &Program, index: u32) -> String {
    if let Some(Constant::String(name)) = program.constants.get(index as usize) {
        format!(
            "\"{}\"",
            name.to_scalar_string()
                .unwrap_or_else(|_| "<non-scalar>".into())
        )
    } else {
        format!("#{index}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn facts_baseline_serialization_round_trip() {
        let mut baseline = FactsBaseline::new();
        let target_facts = ProgramFacts {
            world: ClosedWorldVerdict::Closed,
            modules: vec![ModuleFactsReport {
                path: "bench/cpu/fib.mjs".to_string(),
                immutable_bindings: vec!["fib".to_string()],
                functions: vec![FunctionFacts {
                    name: Some("fib".to_string()),
                    parameter_types: vec![TypeGuess::Number],
                    return_type: TypeGuess::Number,
                    pure: true,
                    can_throw: false,
                    reads_arguments: false,
                    returned_sites: vec![1],
                    sites: vec![SiteFact {
                        id: 1,
                        kind: "Call".to_string(),
                        detail: None,
                    }],
                }],
            }],
            handlers: vec![],
        };
        baseline
            .benchmarks
            .insert("fib".to_string(), target_facts.clone());

        let json_str = baseline
            .to_json_string()
            .expect("serializes to json string");
        assert!(json_str.contains("\"fib\""));
        assert!(json_str.contains("\"bench/cpu/fib.mjs\""));

        let loaded =
            FactsBaseline::from_json_str(&json_str).expect("deserializes from json string");
        assert_eq!(baseline, loaded);
        assert!(baseline.compare(&loaded).is_empty());
    }

    #[test]
    fn module_sizes_format_sorts_and_marks_dropped() {
        let sizes = vec![
            ModuleSizeFacts {
                path: "small.mjs".to_string(),
                live: true,
                total_instructions: 10,
                total_functions: 1,
                retained_instructions: 10,
                retained_functions: 1,
            },
            ModuleSizeFacts {
                path: "big.mjs".to_string(),
                live: true,
                total_instructions: 100,
                total_functions: 5,
                retained_instructions: 60,
                retained_functions: 3,
            },
            ModuleSizeFacts {
                path: "dropped.mjs".to_string(),
                live: false,
                total_instructions: 40,
                total_functions: 2,
                retained_instructions: 0,
                retained_functions: 0,
            },
        ];
        let text = format_module_sizes(&sizes);
        assert!(text.contains("Module Sizes"), "{text}");
        assert!(text.contains("2 live modules, 1 dropped"), "{text}");
        assert!(text.contains("70/150 instructions"), "{text}");
        assert!(text.contains("4/8 functions"), "{text}");
        let big = text.find("big.mjs").expect("big module listed");
        let small = text.find("small.mjs").expect("small module listed");
        assert!(big < small, "largest contributor reads first:\n{text}");
        assert!(
            text.contains("dropped (0/40 instr, 0/2 fns)  dropped.mjs"),
            "{text}"
        );
    }

    #[test]
    fn demanded_members_format_reports_flags() {
        let demanded = DemandedMembers {
            names: BTreeSet::from(["log".to_string(), "push".to_string()]),
            builtins: BTreeSet::from(["SetTimeout".to_string()]),
            globals: BTreeSet::from(["Temporal".to_string()]),
            has_computed_access: true,
            has_open_world: false,
        };
        let text = format_demanded_members(&demanded);
        assert!(text.contains("Demanded Members"), "{text}");
        assert!(
            text.contains("2 distinct names, 1 builtins, 1 globals"),
            "{text}"
        );
        assert!(text.contains("computed access: true"), "{text}");
        assert!(text.contains("open world: false"), "{text}");
        assert!(text.contains("  log"), "{text}");
        assert!(text.contains("  SetTimeout"), "{text}");
        assert!(text.contains("  Temporal"), "{text}");
    }

    #[test]
    fn method_registry_covers_every_group() {
        let mut used = 0u32;
        for method in crate::prelude::METHOD_IDS {
            used |= method.groups;
        }
        let known = CORE_METHOD_GROUPS | PRUNABLE_METHOD_GROUPS;
        assert_eq!(
            used & !known,
            0,
            "method table uses group bits this module does not classify: {used:#x}"
        );
        assert_eq!(
            CORE_METHOD_GROUPS & PRUNABLE_METHOD_GROUPS,
            0,
            "a group cannot be both core and prunable"
        );
    }

    #[test]
    fn program_facts_path_normalization() {
        let root = PathBuf::from("/home/user/project");
        let mut facts = ProgramFacts {
            world: ClosedWorldVerdict::Closed,
            modules: vec![
                ModuleFactsReport {
                    path: "/home/user/project/b.js".to_string(),
                    immutable_bindings: vec!["z".to_string(), "a".to_string()],
                    functions: vec![],
                },
                ModuleFactsReport {
                    path: "/home/user/project/a.js".to_string(),
                    immutable_bindings: vec![],
                    functions: vec![],
                },
            ],
            handlers: vec![],
        };

        facts.normalize_paths(&root);
        assert_eq!(facts.modules[0].path, "a.js");
        assert_eq!(facts.modules[1].path, "b.js");
        assert_eq!(
            facts.modules[1].immutable_bindings,
            vec!["a".to_string(), "z".to_string()]
        );
    }

    #[test]
    fn program_facts_compare_detects_changes() {
        let base = ProgramFacts {
            world: ClosedWorldVerdict::Closed,
            modules: vec![ModuleFactsReport {
                path: "main.js".to_string(),
                immutable_bindings: vec!["foo".to_string()],
                functions: vec![FunctionFacts {
                    name: Some("calc".to_string()),
                    parameter_types: vec![TypeGuess::Unknown],
                    return_type: TypeGuess::Unknown,
                    pure: false,
                    can_throw: true,
                    reads_arguments: false,
                    returned_sites: vec![],
                    sites: vec![],
                }],
            }],
            handlers: vec![],
        };

        let mut modified = base.clone();
        modified.world = ClosedWorldVerdict::Open {
            reasons: vec!["dynamic import".to_string()],
        };
        modified.modules[0].functions[0].return_type = TypeGuess::Number;

        let diffs = modified.compare(&base);
        assert_eq!(diffs.len(), 2);
        assert!(diffs[0].contains("world verdict changed"));
        assert!(diffs[1].contains("return type changed"));
    }

    #[test]
    fn handler_facts_extraction_and_explain() {
        let constants = vec![
            Constant::String("url".to_string().into()),
            Constant::String("method".to_string().into()),
            Constant::String("pathname".to_string().into()),
            Constant::String("writeHead".to_string().into()),
            Constant::String("end".to_string().into()),
            Constant::Number(200.0f64.to_bits()),
            Constant::String("ok".to_string().into()),
        ];

        let handler_proto = FunctionProto {
            inferred_name: None,
            asynchronous: false,
            name: Some("http_handler".to_string()),
            arrow: true,
            generator: false,
            async_generator: false,
            is_method: false,
            is_class: false,
            parameter_count: 2,
            mapped_arguments: false,
            declared_length: 2,
            local_count: 4,
            prologue_end: 0,
            code: vec![
                Instruction::LoadLocal(0),   // req
                Instruction::GetProperty(0), // .url
                Instruction::GetProperty(2), // .pathname
                Instruction::Pop,
                Instruction::LoadLocal(0),   // req
                Instruction::GetProperty(1), // .method
                Instruction::Pop,
                Instruction::LoadLocal(1),                    // res
                Instruction::Const(5),                        // 200
                Instruction::CallMethod { name: 3, argc: 1 }, // .writeHead(200)
                Instruction::Pop,
                Instruction::LoadLocal(1),                    // res
                Instruction::Const(6),                        // "ok"
                Instruction::CallMethod { name: 4, argc: 1 }, // .end("ok")
                Instruction::Pop,
            ],
            upvalues: Vec::new(),
            span: kiln_ir::Span::default(),
            type_annotations: None,
        };

        let program = Program {
            source: "".into(),
            version: kiln_ir::BYTECODE_VERSION,
            constants,
            code: vec![],
            local_count: 0,
            functions: vec![handler_proto],
            template_sites: Vec::new(),
        };

        let facts = analyze_program(&program, Some("server.mjs"));
        assert_eq!(facts.handlers.len(), 1);
        let h = &facts.handlers[0];
        assert_eq!(h.function_name.as_deref(), Some("http_handler"));
        assert_eq!(h.request_members_read, vec!["method", "url"]);
        assert!(h.request_members_omitted.contains(&"headers".to_string()));
        assert_eq!(h.response_members_read, vec!["end", "writeHead"]);
        assert!(
            h.response_members_omitted
                .contains(&"statusCode".to_string())
        );
        assert!(h.url_specialized);
        assert_eq!(h.instructions_per_request, 15);

        let explain = facts.format_explain();
        assert!(explain.contains("Handler Specialization:"));
        assert!(explain.contains("Handler http_handler:"));
        assert!(explain.contains("Request members read: [method, url]"));
        assert!(explain.contains("Response members read: [end, writeHead]"));
        assert!(explain.contains("URL specialized: true"));
        assert!(explain.contains("JS instructions per request: 15"));
    }
}
