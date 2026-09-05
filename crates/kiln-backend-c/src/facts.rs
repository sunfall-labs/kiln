//! Facts analysis and explanation over programs and linked module graphs.
//!
//! Provides whole-program fixpoint information including:
//! - Closed-world vs open-world verdict
//! - Immutable module bindings across the graph
//! - Per-function summaries with inferred parameter and return types
//! - Allocation, call, and property access sites

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

use kiln_ir::{Constant, FunctionProto, Program};
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

/// Whole-program fixpoint facts over a linked graph or standalone program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramFacts {
    pub world: ClosedWorldVerdict,
    pub modules: Vec<ModuleFactsReport>,
}

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

    ProgramFacts { world, modules }
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

    ProgramFacts {
        world,
        modules: vec![module],
    }
}

fn analyze_program_functions(program: &Program) -> Vec<FunctionFacts> {
    let mut functions = Vec::new();

    // Analyze top-level code as a function
    if !program.code.is_empty() {
        let top_level_proto = FunctionProto {
            asynchronous: false,
            name: Some("<top-level>".to_string()),
            arrow: false,
            generator: false,
            async_generator: false,
            parameter_count: 0,
            mapped_arguments: false,
            declared_length: 0,
            local_count: program.local_count,
            prologue_end: 0,
            code: program.code.clone(),
            upvalues: Vec::new(),
            span: kiln_ir::Span::default(),
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
        format!("\"{name}\"")
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
}
