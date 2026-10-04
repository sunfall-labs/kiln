//! Local-slot liveness at generator suspension points, shared by the native emitter and the
//! interpreter's collector.

use crate::graph::{BlockId, FunctionGraph, GraphOp, Terminator, ValueId};
use kiln_ir::{FunctionProto, Instruction, Program};
use std::collections::{HashMap, HashSet};

/// Local slots in a suspended generated function are part of its traced GC frame. Keep only
/// the locals the resume CFG can still read; otherwise a consumed iterator result (and every
/// value reachable through it) remains alive until the generator finishes. The graph's normal
/// value liveness is SSA-based and does not cover these frame slots.
pub fn suspended_local_liveness(
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
) -> HashMap<BlockId, HashSet<u32>> {
    let all = || (0..proto.local_count).collect::<HashSet<_>>();
    if proto
        .code
        .iter()
        .any(|op| matches!(op, Instruction::DirectEval(_)))
    {
        return graph.blocks.iter().map(|block| (block.id, all())).collect();
    }

    let upvalue_values: HashSet<ValueId> = graph
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .filter_map(|(value, op)| matches!(op, GraphOp::LoadUpvalue(_)).then_some(*value))
        .collect();
    let cells: HashSet<u32> = graph
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .filter_map(|(_, op)| match op {
            GraphOp::MakeCell(slot) => Some(*slot),
            // Loading an enclosing binding yields its shared cell, not the cell's
            // payload. A copied frame local can therefore still be a cell edge even
            // when this function never creates a cell of its own.
            GraphOp::StoreLocal { slot, value } if upvalue_values.contains(value) => Some(*slot),
            _ => None,
        })
        .collect();
    let mut uses = HashMap::<BlockId, HashSet<u32>>::new();
    let mut defs = HashMap::<BlockId, HashSet<u32>>::new();
    for block in &graph.blocks {
        let block_uses = uses.entry(block.id).or_default();
        let block_defs = defs.entry(block.id).or_default();
        for (_, op) in &block.operations {
            let (use_slot, def_slot) = match op {
                GraphOp::LoadLocal(slot) | GraphOp::LoadCell(slot) => (Some(*slot), None),
                GraphOp::StoreLocal { slot, .. } => (None, Some(*slot)),
                GraphOp::StoreCell { slot, .. } => (Some(*slot), None),
                GraphOp::MakeCell(slot) => (Some(*slot), Some(*slot)),
                GraphOp::MarkUninitialized(slot) if cells.contains(slot) => (Some(*slot), None),
                GraphOp::MarkUninitialized(slot) => (None, Some(*slot)),
                GraphOp::MakeFunction { index, .. } => {
                    for upvalue in &program.functions[*index as usize].upvalues {
                        let slot = match upvalue {
                            kiln_ir::Upvalue::ParentLocal(slot)
                            | kiln_ir::Upvalue::NamedParentLocal { slot, .. } => Some(*slot),
                            kiln_ir::Upvalue::ParentUpvalue(_)
                            | kiln_ir::Upvalue::NamedParentUpvalue { .. } => None,
                        };
                        if let Some(slot) = slot.filter(|slot| !block_defs.contains(slot)) {
                            block_uses.insert(slot);
                        }
                    }
                    (None, None)
                }
                GraphOp::Bridge {
                    instruction: Instruction::LoadArguments,
                    ..
                } if proto.mapped_arguments => {
                    for slot in 0..proto.parameter_count {
                        if !block_defs.contains(&slot) {
                            block_uses.insert(slot);
                        }
                    }
                    (None, None)
                }
                GraphOp::Bridge {
                    instruction: Instruction::DirectEval(_),
                    ..
                } => {
                    return graph.blocks.iter().map(|block| (block.id, all())).collect();
                }
                _ => (None, None),
            };
            if let Some(slot) = use_slot.filter(|slot| !block_defs.contains(slot)) {
                block_uses.insert(slot);
            }
            if let Some(slot) = def_slot {
                block_defs.insert(slot);
            }
        }
    }

    let mut live_in = graph
        .blocks
        .iter()
        .map(|block| (block.id, HashSet::<u32>::new()))
        .collect::<HashMap<_, _>>();
    let mut changed = true;
    while changed {
        changed = false;
        for block in graph.blocks.iter().rev() {
            let mut normal_out = HashSet::new();
            for successor in &block.successors {
                if let Some(incoming) = live_in.get(successor) {
                    normal_out.extend(incoming.iter().copied());
                }
            }
            let mut incoming = uses.get(&block.id).cloned().unwrap_or_default();
            incoming.extend(normal_out.into_iter().filter(|slot| {
                !defs
                    .get(&block.id)
                    .is_some_and(|defined| defined.contains(slot))
            }));
            // Any operation in the block may throw before a later StoreLocal takes effect.
            // Handler inputs therefore flow around block definitions rather than through the
            // normal edge's kill set.
            if let Some(handler) = block.exception_handler
                && let Some(handler_live) = live_in.get(&handler)
            {
                incoming.extend(handler_live.iter().copied());
            }
            if live_in.get(&block.id) != Some(&incoming) {
                live_in.insert(block.id, incoming);
                changed = true;
            }
        }
    }

    graph
        .blocks
        .iter()
        .filter_map(|block| {
            let Terminator::Suspend {
                resume, delegate, ..
            } = &block.terminator
            else {
                return None;
            };
            let mut live = live_in.get(resume).cloned().unwrap_or_default();
            if let Some((_, exit, _)) = delegate {
                live.extend(live_in.get(exit).into_iter().flatten().copied());
            }
            // `throw()` and `return()` can enter the active catch/finally chain at a
            // suspension. The nearest recorded handler and its outer handlers contribute
            // their locals even if the normal resume edge does not use them.
            if let Some(handler) = block.exception_handler {
                live.extend(live_in.get(&handler).into_iter().flatten().copied());
            }
            // Cell slots can be shared with escaped closures and participate in the
            // generated frame's external-owner ledger. Keep their frame edge even when
            // this resume path has no direct LoadCell: an independently-created closure
            // may outlive the activation and the frame remains an explicit owner while
            // suspended.
            live.extend(cells.iter().copied());
            Some((block.id, live))
        })
        .collect()
}
