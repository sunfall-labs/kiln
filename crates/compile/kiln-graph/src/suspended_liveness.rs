//! Local-slot liveness at generator suspension points, shared by the native emitter and the
//! interpreter's collector.

use crate::graph::{BlockId, FunctionGraph, GraphOp, Terminator, ValueId};
use kiln_ir::{FunctionProto, Instruction, Program};
use std::collections::{HashMap, HashSet, VecDeque};

const MAX_SUSPENDED_LOCAL_STATE_CELLS: usize = 262_144;
const MAX_SUSPENDED_LOCAL_ANALYSIS_WORK: usize = 8_000_000;

#[derive(Clone, Copy)]
struct SuspendedLocalAnalysisLimits {
    state_cells: usize,
    work: usize,
}

const PRODUCTION_SUSPENDED_LOCAL_LIMITS: SuspendedLocalAnalysisLimits =
    SuspendedLocalAnalysisLimits {
        state_cells: MAX_SUSPENDED_LOCAL_STATE_CELLS,
        work: MAX_SUSPENDED_LOCAL_ANALYSIS_WORK,
    };

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

/// Frame-local slots that may hold a value when each suspension is reached.
///
/// Unlike [`suspended_local_liveness`], this is a forward may-analysis: it starts from the
/// initialized generator frame, follows the resume CFG, and forgets slots that the suspension
/// cleanup is proven to clear. Callers may omit a cleanup store only for slots absent from this
/// set. The backward live set remains authoritative; this analysis never decides that a live
/// slot can be cleared.
pub fn suspended_local_clear_slots(
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
) -> HashMap<BlockId, HashSet<u32>> {
    suspended_local_clear_slots_with_limits(
        graph,
        proto,
        program,
        PRODUCTION_SUSPENDED_LOCAL_LIMITS,
    )
}

fn suspended_local_clear_slots_with_limits(
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    limits: SuspendedLocalAnalysisLimits,
) -> HashMap<BlockId, HashSet<u32>> {
    let live_at_suspend = suspended_local_liveness(graph, proto, program);
    let suspends: Vec<_> = graph
        .blocks
        .iter()
        .filter(|block| matches!(block.terminator, Terminator::Suspend { .. }))
        .collect();
    if suspends.is_empty() {
        return HashMap::new();
    }
    // Missing backward facts mean the old emitter omitted cleanup for this suspend. Do not
    // turn an unavailable fact into an empty live set, which could erase a required frame edge.
    if suspends
        .iter()
        .any(|block| !live_at_suspend.contains_key(&block.id))
    {
        return previous_clear_slots(graph, proto.local_count, &live_at_suspend);
    }

    let conservative = proto.mapped_arguments
        || proto
            .code
            .iter()
            .any(|op| matches!(op, Instruction::DirectEval(_)));
    if conservative {
        return previous_clear_slots(graph, proto.local_count, &live_at_suspend);
    }

    let local_count = usize::try_from(proto.local_count).unwrap_or(usize::MAX);
    if graph
        .blocks
        .len()
        .checked_mul(local_count.max(1))
        .is_none_or(|cells| cells > limits.state_cells)
    {
        return previous_clear_slots(graph, proto.local_count, &live_at_suspend);
    }

    // A new generator frame is initialized to undefined, then receives all arguments up to
    // local_count (including arguments beyond the formal parameter count). Treat every slot as
    // possibly populated on first entry; later suspension edges remove slots that were cleared.
    let mut may_populated_in: HashMap<BlockId, HashSet<u32>> = graph
        .blocks
        .iter()
        .map(|block| (block.id, HashSet::new()))
        .collect();
    if let Some(entry) = may_populated_in.get_mut(&graph.entry_block) {
        entry.extend(0..proto.local_count);
    }

    // Resolve successor IDs once. FunctionGraph::block scans the block vector, which turns a
    // worklist over a large resume CFG into repeated linear searches.
    let blocks_by_id: HashMap<_, _> = graph.blocks.iter().map(|block| (block.id, block)).collect();
    if blocks_by_id.len() != graph.blocks.len() || !blocks_by_id.contains_key(&graph.entry_block) {
        return previous_clear_slots(graph, proto.local_count, &live_at_suspend);
    }

    let mut reachable = HashSet::from([graph.entry_block]);
    let mut queued = HashSet::from([graph.entry_block]);
    let mut work = VecDeque::from([graph.entry_block]);
    let mut analysis_work = 0usize;
    while let Some(block_id) = work.pop_front() {
        queued.remove(&block_id);
        let Some(block) = blocks_by_id.get(&block_id).copied() else {
            continue;
        };
        let Some(incoming_state) = may_populated_in.get(&block.id) else {
            return previous_clear_slots(graph, proto.local_count, &live_at_suspend);
        };
        if !charge_analysis_work(
            &mut analysis_work,
            incoming_state
                .len()
                .saturating_add(block.operations.len())
                .saturating_add(block.successors.len())
                .saturating_add(usize::from(block.exception_handler.is_some()))
                .max(1),
            limits.work,
        ) {
            return previous_clear_slots(graph, proto.local_count, &live_at_suspend);
        }
        let mut populated = incoming_state.clone();
        for (_, operation) in &block.operations {
            if let Some(slot) = local_slot_write(operation)
                && slot < proto.local_count
            {
                populated.insert(slot);
            }
        }

        let after_suspend = match &block.terminator {
            Terminator::Suspend { .. } => {
                let live = live_at_suspend.get(&block.id).cloned().unwrap_or_default();
                if !charge_analysis_work(
                    &mut analysis_work,
                    populated.len().saturating_add(live.len()),
                    limits.work,
                ) {
                    return previous_clear_slots(graph, proto.local_count, &live_at_suspend);
                }
                Some(
                    populated
                        .intersection(&live)
                        .copied()
                        .collect::<HashSet<_>>(),
                )
            }
            _ => None,
        };

        for successor in block
            .successors
            .iter()
            .chain(block.exception_handler.iter())
        {
            let edge = match after_suspend.as_ref() {
                // An exception can enter the handler before a block reaches its suspension
                // cleanup. Propagate the unclean state on every handler edge. If the handler
                // aliases a resume edge, this only overestimates and emits extra clears.
                Some(cleaned) if Some(*successor) != block.exception_handler => cleaned,
                _ => &populated,
            };
            let Some(incoming) = may_populated_in.get_mut(successor) else {
                continue;
            };
            if !charge_analysis_work(&mut analysis_work, edge.len().max(1), limits.work) {
                return previous_clear_slots(graph, proto.local_count, &live_at_suspend);
            }
            let old_len = incoming.len();
            incoming.extend(edge.iter().copied());
            let newly_reachable = reachable.insert(*successor);
            if (newly_reachable || incoming.len() != old_len) && queued.insert(*successor) {
                work.push_back(*successor);
            }
        }
    }

    let mut clear_at_suspend = HashMap::with_capacity(suspends.len());
    for block in suspends {
        let mut populated = may_populated_in.get(&block.id).cloned().unwrap_or_default();
        for (_, operation) in &block.operations {
            if let Some(slot) = local_slot_write(operation)
                && slot < proto.local_count
            {
                populated.insert(slot);
            }
        }
        let live = live_at_suspend.get(&block.id).cloned().unwrap_or_default();
        if !charge_analysis_work(
            &mut analysis_work,
            populated.len().saturating_add(live.len()),
            limits.work,
        ) {
            return previous_clear_slots(graph, proto.local_count, &live_at_suspend);
        }
        clear_at_suspend.insert(block.id, populated.difference(&live).copied().collect());
    }
    clear_at_suspend
}

fn local_slot_write(operation: &GraphOp) -> Option<u32> {
    match operation {
        GraphOp::StoreLocal { slot, .. }
        | GraphOp::MakeCell(slot)
        | GraphOp::MarkUninitialized(slot) => Some(*slot),
        _ => None,
    }
}

fn charge_analysis_work(work: &mut usize, cost: usize, maximum: usize) -> bool {
    let Some(total) = work.checked_add(cost) else {
        return false;
    };
    if total > maximum {
        return false;
    }
    *work = total;
    true
}

fn previous_clear_slots(
    graph: &FunctionGraph,
    local_count: u32,
    live_at_suspend: &HashMap<BlockId, HashSet<u32>>,
) -> HashMap<BlockId, HashSet<u32>> {
    graph
        .blocks
        .iter()
        .filter(|block| matches!(block.terminator, Terminator::Suspend { .. }))
        .filter_map(|block| {
            let live = live_at_suspend.get(&block.id)?;
            let clear = (0..local_count)
                .filter(|slot| !live.contains(slot))
                .collect();
            Some((block.id, clear))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::BasicBlock;
    use kiln_ir::{Span, Upvalue};

    fn generator_proto(local_count: u32) -> FunctionProto {
        FunctionProto {
            asynchronous: false,
            name: None,
            inferred_name: None,
            arrow: false,
            generator: true,
            async_generator: false,
            is_method: false,
            is_class: false,
            parameter_count: 0,
            mapped_arguments: false,
            declared_length: 0,
            local_count,
            prologue_end: 0,
            code: Vec::new(),
            upvalues: Vec::<Upvalue>::new(),
            span: Span::default(),
            type_annotations: None,
        }
    }

    fn suspend(resume: u32) -> Terminator {
        Terminator::Suspend {
            value: ValueId(0),
            resume: BlockId(resume),
            spilled: Vec::new(),
            pc: 0,
            awaiting: false,
            delegate: None,
        }
    }

    fn block(
        id: u32,
        operations: Vec<(ValueId, GraphOp)>,
        terminator: Terminator,
        successors: Vec<u32>,
    ) -> BasicBlock {
        BasicBlock {
            id: BlockId(id),
            operations,
            terminator,
            predecessors: Vec::new(),
            successors: successors.into_iter().map(BlockId).collect(),
            exception_handler: None,
        }
    }

    fn retained_then_dead_graph() -> FunctionGraph {
        let mut graph = FunctionGraph::new(None, 0, 3);
        graph.blocks = vec![
            block(
                0,
                vec![(
                    ValueId(1),
                    GraphOp::StoreLocal {
                        slot: 0,
                        value: ValueId(2),
                    },
                )],
                suspend(1),
                vec![1],
            ),
            block(
                1,
                vec![(ValueId(3), GraphOp::LoadLocal(0))],
                suspend(2),
                vec![2],
            ),
            block(2, Vec::new(), Terminator::Return(None), Vec::new()),
        ];
        graph
    }

    #[test]
    fn suspended_local_budget_fallbacks_keep_legacy_clears() {
        let graph = retained_then_dead_graph();
        let proto = generator_proto(3);
        let program = Program::default();
        let expected_legacy = HashMap::from([
            (BlockId(0), HashSet::from([1, 2])),
            (BlockId(1), HashSet::from([0, 1, 2])),
        ]);

        // Normal bounded analysis carries slot 0 across the first yield, then clears it once it
        // becomes dead. The legacy fallback intentionally re-clears all dead slots each time.
        let optimized = suspended_local_clear_slots_with_limits(
            &graph,
            &proto,
            &program,
            SuspendedLocalAnalysisLimits {
                state_cells: 64,
                work: 1024,
            },
        );
        assert_eq!(
            optimized,
            HashMap::from([
                (BlockId(0), HashSet::from([1, 2])),
                (BlockId(1), HashSet::from([0])),
            ])
        );

        let state_budget_fallback = suspended_local_clear_slots_with_limits(
            &graph,
            &proto,
            &program,
            SuspendedLocalAnalysisLimits {
                state_cells: 1,
                work: 1024,
            },
        );
        assert_eq!(state_budget_fallback, expected_legacy);

        let work_budget_fallback = suspended_local_clear_slots_with_limits(
            &graph,
            &proto,
            &program,
            SuspendedLocalAnalysisLimits {
                state_cells: 64,
                work: 0,
            },
        );
        assert_eq!(work_budget_fallback, expected_legacy);
    }

    #[test]
    fn suspended_local_missing_entry_falls_back_to_legacy_clears() {
        let mut graph = retained_then_dead_graph();
        graph.entry_block = BlockId(99);
        let clear = suspended_local_clear_slots_with_limits(
            &graph,
            &generator_proto(3),
            &Program::default(),
            PRODUCTION_SUSPENDED_LOCAL_LIMITS,
        );

        assert_eq!(
            clear,
            HashMap::from([
                (BlockId(0), HashSet::from([1, 2])),
                (BlockId(1), HashSet::from([0, 1, 2])),
            ])
        );
    }
}
