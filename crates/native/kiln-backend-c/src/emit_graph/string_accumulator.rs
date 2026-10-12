//! Private loop-carried string storage. The proof rejects every observation inside the loop;
//! leaving the loop drops ownership before a value can escape. This is representation lowering,
//! not a different implementation of JavaScript addition or string coercion.
use crate::graph::{BlockId, FunctionGraph, GraphOp, Terminator, ValueId};
use kiln_ir::{BinaryOp, Constant, FunctionProto, Instruction, Program};
use std::collections::{HashMap, HashSet};

pub(super) struct Accumulator {
    pub phi: ValueId,
    pub appends: HashSet<ValueId>,
    pub blocks: HashSet<BlockId>,
}

fn reachable(graph: &FunctionGraph, start: BlockId, reverse: bool) -> HashSet<BlockId> {
    let mut result = HashSet::new();
    let mut pending = vec![start];
    while let Some(id) = pending.pop() {
        if !result.insert(id) {
            continue;
        }
        if let Some(block) = graph.block(id) {
            pending.extend(if reverse {
                &block.predecessors
            } else {
                &block.successors
            });
        }
    }
    result
}

pub(super) fn analyze(
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
) -> Vec<Accumulator> {
    // Suspension, eval and observable script globals cannot keep a private mutable value.
    if proto.generator
        || proto.asynchronous
        || program.has_direct_eval()
        || proto.code.iter().any(|i| {
            matches!(
                i,
                Instruction::DeclareGlobalBinding { .. } | Instruction::WithHas(_)
            )
        })
    {
        return Vec::new();
    }
    let defs: HashMap<_, _> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(v, op)| (*v, op)))
        .collect();
    let mut result = Vec::new();
    for header in &graph.blocks {
        for &(phi, ref op) in &header.operations {
            let GraphOp::Phi { inputs } = op else {
                continue;
            };
            let mut appends = HashSet::new();
            let mut initial = false;
            let mut valid = true;
            for (_, input) in inputs {
                match defs.get(input).copied() {
                    Some(GraphOp::Const {
                        constant: Constant::String(_),
                        ..
                    }) => initial = true,
                    Some(GraphOp::Binary {
                        op: BinaryOp::Add,
                        left,
                        right,
                    }) if *left == phi && *right != phi => {
                        appends.insert(*input);
                    }
                    _ => {
                        valid = false;
                        break;
                    }
                }
            }
            if !valid || !initial || appends.is_empty() {
                continue;
            }
            let forward = reachable(graph, header.id, false);
            let backward = reachable(graph, header.id, true);
            let blocks: HashSet<_> = forward.intersection(&backward).copied().collect();
            // A plain merge is not an accumulation loop.
            if !inputs
                .iter()
                .any(|(pred, value)| blocks.contains(pred) && appends.contains(value))
            {
                continue;
            }
            let slots: HashSet<_> = graph
                .blocks
                .iter()
                .flat_map(|b| &b.operations)
                .filter_map(|(_, op)| match op {
                    GraphOp::StoreLocal { slot, value } if appends.contains(value) => Some(*slot),
                    _ => None,
                })
                .collect();
            if slots.len() != 1 {
                continue;
            }
            let slot = *slots.iter().next().unwrap();
            if slot < proto.parameter_count || graph.blocks.iter().flat_map(|b| &b.operations).any(|(_, op)| matches!(op,
                GraphOp::MakeCell(s) | GraphOp::LoadCell(s) | GraphOp::StoreCell { slot: s, .. } if *s == slot)) { continue; }
            for block in &graph.blocks {
                for &(value, ref op) in &block.operations {
                    for operand in op.operands() {
                        if operand != phi && !appends.contains(&operand) {
                            continue;
                        }
                        let allowed = match op {
                            GraphOp::Binary {
                                op: BinaryOp::Add,
                                left,
                                ..
                            } => {
                                appends.contains(&value)
                                    && *left == operand
                                    && operand == phi
                                    && blocks.contains(&block.id)
                            }
                            GraphOp::Phi { .. } => value == phi,
                            GraphOp::StoreLocal { slot: s, .. } => {
                                *s == slot
                                    && !graph.blocks.iter().any(|b| {
                                        b.operations
                                            .iter()
                                            .any(|(_, user)| user.operands().contains(&value))
                                    })
                            }
                            _ => false,
                        };
                        // Outside observations of the *current phi* freeze storage at the exit.
                        // An append result exposed elsewhere can retain an earlier iteration.
                        if !allowed && (blocks.contains(&block.id) || operand != phi) {
                            valid = false;
                        }
                    }
                }
                // A terminator is an observation too, including a retained append result.
                let observed: Vec<ValueId> = match &block.terminator {
                    Terminator::Branch { condition, .. }
                    | Terminator::BranchNotNullish { condition, .. } => vec![*condition],
                    Terminator::Return(v) => v.iter().copied().collect(),
                    Terminator::Throw(v) => vec![*v],
                    Terminator::Suspend { .. } => {
                        valid = false;
                        Vec::new()
                    }
                    _ => Vec::new(),
                };
                if observed.iter().any(|v| {
                    (*v == phi || appends.contains(v)) && (blocks.contains(&block.id) || *v != phi)
                }) {
                    valid = false;
                }
            }
            if valid {
                result.push(Accumulator {
                    phi,
                    appends,
                    blocks,
                });
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidates(source: &str) -> usize {
        let hir = kiln_frontend_oxc::parse("fixture/accumulator.ts", source);
        let program = kiln_ir::compile_module(&hir).unwrap().program;
        let proto = crate::entry_proto(&program);
        let graph = crate::graph::build_function_graph(&proto, &program).unwrap();
        let result = analyze(&graph, &proto, &program);
        if result.is_empty() {
            eprintln!("{source}\n{graph:#?}");
        }
        result.len()
    }
    #[test]
    fn string_accumulator_private_loop_is_recognized() {
        assert_eq!(
            candidates("let s = ''; for (let i=0;i<100;i++) s += 'x'; console.log(s);"),
            1
        );
        assert_eq!(
            candidates(
                "let s = ''; for (let i=0;i<100;i++) { if (i%2) s += 'x'; else s += 'y'; } console.log(s);"
            ),
            0 // Merged conditional back edges are deliberately outside the first proof.
        );
    }
    #[test]
    fn string_accumulator_observed_versions_stay_immutable() {
        for source in [
            "let s=''; let old=''; for(let i=0;i<10;i++){ old=s; s+='x'; } console.log(old,s);",
            "let s=''; const snapshots=[]; for(let i=0;i<10;i++){ snapshots.push(s); s+='x'; } console.log(snapshots);",
            "let s=''; for(let i=0;i<10;i++){ console.log(s); s+='x'; }",
            "let s=''; function read(){ return s; } for(let i=0;i<10;i++) s+=read(); console.log(s);",
            "let s=''; for(let i=0;i<10;i++){ eval('s'); s+='x'; }",
            "let n=0; for(let i=0;i<10;i++) n+=i; console.log(n);",
        ] {
            assert_eq!(candidates(source), 0, "{source}");
        }
    }
}
