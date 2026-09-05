//! Facts analysis and explanation over programs and linked module graphs.
//!
//! Provides whole-program fixpoint information including:
//! - Closed-world vs open-world verdict
//! - Immutable module bindings across the graph
//! - Per-function summaries with inferred parameter and return types
//! - Allocation, call, and property access sites

use std::fmt::Write;

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
    pub detail: Option<String>,
}

/// Inferred facts for a compiled function or top-level body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionFacts {
    pub name: Option<String>,
    pub parameter_types: Vec<TypeGuess>,
    pub return_type: TypeGuess,
    pub pure: bool,
    pub can_throw: bool,
    pub reads_arguments: bool,
    pub returned_sites: Vec<u32>,
    pub sites: Vec<SiteFact>,
}

/// Inferred facts for a single module within a linked graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleFactsReport {
    pub path: String,
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
}

/// Analyze a complete linked `ModuleGraph` and extract whole-program facts.
pub fn analyze_graph(graph: &ModuleGraph) -> ProgramFacts {
    let world = graph.closed_world_verdict();
    let mut modules = Vec::with_capacity(graph.module_count());

    for node in graph.modules() {
        let path = node.path.display().to_string();
        let immutable_bindings = node.program.link_facts.immutable_bindings.clone();
        let functions = analyze_program_functions(&node.program.program);
        modules.push(ModuleFactsReport {
            path,
            immutable_bindings,
            functions,
        });
    }

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
        let graph = build_function_graph(&top_level_proto, program);
        functions.push(extract_function_facts(&graph, program));
    }

    // Analyze each declared function
    for proto in &program.functions {
        let graph = build_function_graph(proto, program);
        functions.push(extract_function_facts(&graph, program));
    }

    functions
}

fn extract_function_facts(graph: &FunctionGraph, program: &Program) -> FunctionFacts {
    let summary = summarize_graph(graph);
    let mut sites = Vec::new();

    for block in &graph.blocks {
        for (_value_id, op, _graph_type) in &block.operations {
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
                GraphOp::CallComputed { site, .. } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "CallComputed".to_string(),
                        detail: None,
                    });
                }
                GraphOp::HostCall { op, site, .. } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "HostCall".to_string(),
                        detail: Some(format!("{op:?}")),
                    });
                }
                GraphOp::NewArray { site, .. } => {
                    sites.push(SiteFact {
                        id: site.0,
                        kind: "NewArray".to_string(),
                        detail: None,
                    });
                }
                GraphOp::DirectEval { site_idx, .. } => {
                    sites.push(SiteFact {
                        id: *site_idx,
                        kind: "DirectEval".to_string(),
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
