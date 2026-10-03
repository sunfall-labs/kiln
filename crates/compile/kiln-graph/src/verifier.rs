//! Graph verifier: validates SSA dominance, control flow validity, type consistency,
//! and architectural invariants (ADR 0056 / Step 1.2).
//!
//! A fast wrong answer costs more than the speed it bought. The verifier runs in debug builds
//! and CI gate to guarantee no corrupted or ill-formed graph ever reaches C emission.

use crate::graph::{BasicBlock, BlockId, FunctionGraph, GraphOp, Terminator, ValueId};
use std::collections::HashMap;

/// Errors detected by graph verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifierError {
    EmptyGraph,
    MissingEntryBlock(BlockId),
    DuplicateBlockId(BlockId),
    UnknownBlock(BlockId),
    AsymmetricEdge {
        from: BlockId,
        to: BlockId,
        detail: &'static str,
    },
    DuplicateValueDefinition(ValueId),
    UseBeforeDef {
        value: ValueId,
        user_block: BlockId,
    },
    DominanceViolation {
        value: ValueId,
        def_block: BlockId,
        use_block: BlockId,
    },
    InvalidTerminator {
        block: BlockId,
        detail: String,
    },
    InvalidExceptionHandler {
        block: BlockId,
        target: BlockId,
    },
    InvalidPhi {
        block: BlockId,
        detail: String,
    },
}

impl std::fmt::Display for VerifierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyGraph => write!(f, "graph contains no basic blocks"),
            Self::MissingEntryBlock(bid) => write!(f, "entry block {bid:?} is missing from graph"),
            Self::DuplicateBlockId(bid) => write!(f, "duplicate block id {bid:?}"),
            Self::UnknownBlock(bid) => write!(f, "reference to unknown block id {bid:?}"),
            Self::AsymmetricEdge { from, to, detail } => {
                write!(
                    f,
                    "asymmetric CFG edge between {from:?} and {to:?}: {detail}"
                )
            }
            Self::DuplicateValueDefinition(vid) => {
                write!(f, "value {vid:?} is defined multiple times (violating SSA)")
            }
            Self::UseBeforeDef { value, user_block } => {
                write!(
                    f,
                    "value {value:?} is used in block {user_block:?} before being defined"
                )
            }
            Self::DominanceViolation {
                value,
                def_block,
                use_block,
            } => {
                write!(
                    f,
                    "dominance violation: value {value:?} defined in {def_block:?} does not dominate use in {use_block:?}"
                )
            }
            Self::InvalidTerminator { block, detail } => {
                write!(f, "invalid terminator in block {block:?}: {detail}")
            }
            Self::InvalidExceptionHandler { block, target } => {
                write!(
                    f,
                    "invalid exception handler for block {block:?} targeting unknown block {target:?}"
                )
            }
            Self::InvalidPhi { block, detail } => {
                write!(f, "invalid Phi node in block {block:?}: {detail}")
            }
        }
    }
}

impl std::error::Error for VerifierError {}

/// Verify a FunctionGraph against all invariants defined in ADR 0056.
pub fn verify_function_graph(graph: &FunctionGraph) -> Result<(), VerifierError> {
    if graph.blocks.is_empty() {
        return Err(VerifierError::EmptyGraph);
    }

    let mut block_map: HashMap<BlockId, &BasicBlock> = HashMap::new();
    for block in &graph.blocks {
        if block_map.insert(block.id, block).is_some() {
            return Err(VerifierError::DuplicateBlockId(block.id));
        }
    }

    if !block_map.contains_key(&graph.entry_block) {
        return Err(VerifierError::MissingEntryBlock(graph.entry_block));
    }

    // 1. Control Flow Symmetry and Valid Successors/Predecessors
    for block in &graph.blocks {
        if let Some(target) = block
            .exception_handler
            .filter(|target| !block_map.contains_key(target))
        {
            return Err(VerifierError::InvalidExceptionHandler {
                block: block.id,
                target,
            });
        }

        for &succ in &block.successors {
            let succ_block = block_map
                .get(&succ)
                .ok_or(VerifierError::UnknownBlock(succ))?;
            if !succ_block.predecessors.contains(&block.id) {
                return Err(VerifierError::AsymmetricEdge {
                    from: block.id,
                    to: succ,
                    detail: "successor does not list predecessor in predecessors list",
                });
            }
        }

        for &pred in &block.predecessors {
            let pred_block = block_map
                .get(&pred)
                .ok_or(VerifierError::UnknownBlock(pred))?;
            if !pred_block.successors.contains(&block.id) {
                return Err(VerifierError::AsymmetricEdge {
                    from: pred,
                    to: block.id,
                    detail: "predecessor does not list successor in successors list",
                });
            }
        }

        // Terminator validity
        match &block.terminator {
            Terminator::Jump(target) => {
                if !block_map.contains_key(target) {
                    return Err(VerifierError::UnknownBlock(*target));
                }
                if !block.successors.contains(target) {
                    return Err(VerifierError::InvalidTerminator {
                        block: block.id,
                        detail: format!("Jump target {target:?} not in successors"),
                    });
                }
            }
            Terminator::Branch {
                condition: _,
                true_block,
                false_block,
            }
            | Terminator::BranchNotNullish {
                condition: _,
                true_block,
                false_block,
            } => {
                if !block_map.contains_key(true_block) {
                    return Err(VerifierError::UnknownBlock(*true_block));
                }
                if !block_map.contains_key(false_block) {
                    return Err(VerifierError::UnknownBlock(*false_block));
                }
                if !block.successors.contains(true_block) || !block.successors.contains(false_block)
                {
                    return Err(VerifierError::InvalidTerminator {
                        block: block.id,
                        detail: format!(
                            "Branch targets ({true_block:?}, {false_block:?}) not in successors"
                        ),
                    });
                }
            }
            Terminator::Suspend {
                resume, delegate, ..
            } => {
                // These are synthetic edges: the body returns, and a later call re-enters at the
                // target through the dispatch switch. The graph still records them so resume
                // phis have complete inputs; the emitter implements the edge with frame loads,
                // never a C goto.
                if !block_map.contains_key(resume) {
                    return Err(VerifierError::UnknownBlock(*resume));
                }
                if let Some((_, exit_block, _)) = delegate
                    && !block_map.contains_key(exit_block)
                {
                    return Err(VerifierError::UnknownBlock(*exit_block));
                }
            }
            Terminator::Return(_) | Terminator::Throw(_) | Terminator::Unreachable => {}
        }
    }

    // 2. Compute Dominator Tree (fixed-point iteration)
    let dominators = crate::dominators::compute(graph, true);

    // 3. SSA Uniqueness and Dominance Check
    let mut def_site: HashMap<ValueId, (BlockId, usize)> = HashMap::new();

    for block in &graph.blocks {
        for (idx, (vid, _)) in block.operations.iter().enumerate() {
            if def_site.insert(*vid, (block.id, idx)).is_some() {
                return Err(VerifierError::DuplicateValueDefinition(*vid));
            }
        }
    }

    let dominates = |def_block: BlockId, use_block: BlockId| -> bool {
        dominators.contains(def_block, use_block)
    };

    for block in &graph.blocks {
        for (op_idx, (_vid, op)) in block.operations.iter().enumerate() {
            let check_operand = |used_val: ValueId| -> Result<(), VerifierError> {
                let &(def_block, def_idx) =
                    def_site.get(&used_val).ok_or(VerifierError::UseBeforeDef {
                        value: used_val,
                        user_block: block.id,
                    })?;

                if def_block == block.id {
                    if def_idx >= op_idx {
                        return Err(VerifierError::UseBeforeDef {
                            value: used_val,
                            user_block: block.id,
                        });
                    }
                } else if !dominates(def_block, block.id) {
                    return Err(VerifierError::DominanceViolation {
                        value: used_val,
                        def_block,
                        use_block: block.id,
                    });
                }
                Ok(())
            };

            match op {
                GraphOp::StoreLocal { value, .. }
                | GraphOp::StoreCell { value, .. }
                | GraphOp::StoreUpvalue { value, .. }
                | GraphOp::StoreGlobal { value, .. }
                | GraphOp::CheckTdz { value, .. }
                | GraphOp::Unary { value, .. }
                | GraphOp::IsReturnSignal(value) => {
                    check_operand(*value)?;
                }
                GraphOp::Binary { left, right, .. } => {
                    check_operand(*left)?;
                    check_operand(*right)?;
                }
                GraphOp::Bridge { operands, .. } => {
                    for operand in operands {
                        check_operand(*operand)?;
                    }
                }
                GraphOp::New { callee, args, .. } => {
                    check_operand(*callee)?;
                    for arg in args {
                        check_operand(*arg)?;
                    }
                }
                GraphOp::NewObject { pairs, .. } => {
                    for (key, value) in pairs {
                        check_operand(*key)?;
                        check_operand(*value)?;
                    }
                }
                GraphOp::GetComputed { object, key, .. } => {
                    check_operand(*object)?;
                    check_operand(*key)?;
                }
                GraphOp::GetProperty { object, .. } => {
                    check_operand(*object)?;
                }
                GraphOp::SetProperty { object, value, .. }
                | GraphOp::SetPropertyStrict { object, value, .. }
                | GraphOp::DefineOwnProperty { object, value, .. } => {
                    check_operand(*object)?;
                    check_operand(*value)?;
                }
                GraphOp::SetComputed {
                    object, key, value, ..
                }
                | GraphOp::SetComputedStrict {
                    object, key, value, ..
                }
                | GraphOp::DefineOwnComputed {
                    object, key, value, ..
                } => {
                    check_operand(*object)?;
                    check_operand(*key)?;
                    check_operand(*value)?;
                }
                GraphOp::Call { callee, args, .. } => {
                    check_operand(*callee)?;
                    for arg in args {
                        check_operand(*arg)?;
                    }
                }
                GraphOp::CallMethod { object, args, .. } => {
                    check_operand(*object)?;
                    for arg in args {
                        check_operand(*arg)?;
                    }
                }
                GraphOp::NewArray { elements: args, .. } => {
                    for arg in args {
                        check_operand(*arg)?;
                    }
                }
                GraphOp::Phi { inputs } => {
                    if inputs.len() != block.predecessors.len()
                        || block
                            .predecessors
                            .iter()
                            .any(|p| inputs.iter().filter(|(b, _)| b == p).count() != 1)
                    {
                        return Err(VerifierError::InvalidPhi {
                            block: block.id,
                            detail: format!(
                                "Phi has {} inputs for {} predecessors",
                                inputs.len(),
                                block.predecessors.len()
                            ),
                        });
                    }
                    for (pred_block, val) in inputs {
                        if !block.predecessors.contains(pred_block) {
                            return Err(VerifierError::InvalidPhi {
                                block: block.id,
                                detail: format!("Phi names non-predecessor block {pred_block:?}"),
                            });
                        }
                        let &(def_block, _) =
                            def_site.get(val).ok_or(VerifierError::UseBeforeDef {
                                value: *val,
                                user_block: block.id,
                            })?;
                        if !dominates(def_block, *pred_block) {
                            return Err(VerifierError::DominanceViolation {
                                value: *val,
                                def_block,
                                use_block: *pred_block,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        // Check terminator operands
        match &block.terminator {
            Terminator::Branch { condition, .. }
            | Terminator::BranchNotNullish { condition, .. } => {
                let &(def_block, _def_idx) =
                    def_site.get(condition).ok_or(VerifierError::UseBeforeDef {
                        value: *condition,
                        user_block: block.id,
                    })?;
                if def_block != block.id && !dominates(def_block, block.id) {
                    return Err(VerifierError::DominanceViolation {
                        value: *condition,
                        def_block,
                        use_block: block.id,
                    });
                }
            }
            Terminator::Return(Some(ret_val)) | Terminator::Throw(ret_val) => {
                let &(def_block, _) = def_site.get(ret_val).ok_or(VerifierError::UseBeforeDef {
                    value: *ret_val,
                    user_block: block.id,
                })?;
                if def_block != block.id && !dominates(def_block, block.id) {
                    return Err(VerifierError::DominanceViolation {
                        value: *ret_val,
                        def_block,
                        use_block: block.id,
                    });
                }
            }
            Terminator::Suspend {
                value,
                spilled,
                delegate,
                ..
            } => {
                let delegate_iter = delegate.as_ref().map(|(iter, _, _)| iter);
                for used in std::iter::once(value)
                    .chain(delegate_iter)
                    .chain(spilled.iter())
                {
                    let &(def_block, _) =
                        def_site.get(used).ok_or(VerifierError::UseBeforeDef {
                            value: *used,
                            user_block: block.id,
                        })?;
                    if def_block != block.id && !dominates(def_block, block.id) {
                        return Err(VerifierError::DominanceViolation {
                            value: *used,
                            def_block,
                            use_block: block.id,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{BasicBlock, BlockId, FunctionGraph, GraphOp, Terminator};
    use kiln_ir::Constant;

    #[test]
    fn valid_graph_passes_verification() {
        let mut graph = FunctionGraph::new(Some("test".into()), 0, 1);
        let b0 = BlockId(0);
        let v0 = graph.alloc_value();

        graph.blocks.push(BasicBlock {
            id: b0,
            operations: vec![(
                v0,
                GraphOp::Const {
                    index: 0,
                    constant: Constant::Number(42),
                },
            )],
            terminator: Terminator::Return(Some(v0)),
            predecessors: vec![],
            successors: vec![],
            exception_handler: None,
        });

        assert!(verify_function_graph(&graph).is_ok());
    }

    #[test]
    fn verifier_rejects_dominance_violation() {
        let mut graph = FunctionGraph::new(Some("dom_violation".into()), 0, 1);
        let b0 = BlockId(0);
        let b1 = BlockId(1);
        let b2 = BlockId(2);

        let v_cond = graph.alloc_value();
        let v_in_b1 = graph.alloc_value();

        // b0 branches to b1 or b2
        graph.blocks.push(BasicBlock {
            id: b0,
            operations: vec![(
                v_cond,
                GraphOp::Const {
                    index: 0,
                    constant: Constant::Boolean(true),
                },
            )],
            terminator: Terminator::Branch {
                condition: v_cond,
                true_block: b1,
                false_block: b2,
            },
            predecessors: vec![],
            successors: vec![b1, b2],
            exception_handler: None,
        });

        // b1 defines v_in_b1
        graph.blocks.push(BasicBlock {
            id: b1,
            operations: vec![(
                v_in_b1,
                GraphOp::Const {
                    index: 1,
                    constant: Constant::Number(100),
                },
            )],
            terminator: Terminator::Return(Some(v_in_b1)),
            predecessors: vec![b0],
            successors: vec![],
            exception_handler: None,
        });

        // b2 illegally uses v_in_b1 without dominance!
        graph.blocks.push(BasicBlock {
            id: b2,
            operations: vec![],
            terminator: Terminator::Return(Some(v_in_b1)),
            predecessors: vec![b0],
            successors: vec![],
            exception_handler: None,
        });

        let result = verify_function_graph(&graph);
        assert!(result.is_err());
        match result.unwrap_err() {
            VerifierError::DominanceViolation {
                value,
                def_block,
                use_block,
            } => {
                assert_eq!(value, v_in_b1);
                assert_eq!(def_block, b1);
                assert_eq!(use_block, b2);
            }
            other => panic!("expected DominanceViolation, got {other:?}"),
        }
    }

    #[test]
    fn verifier_rejects_asymmetric_cf_edges() {
        let mut graph = FunctionGraph::new(Some("asym".into()), 0, 1);
        let b0 = BlockId(0);
        let b1 = BlockId(1);

        // b0 lists b1 as successor, but b1 does not list b0 as predecessor!
        graph.blocks.push(BasicBlock {
            id: b0,
            operations: vec![],
            terminator: Terminator::Jump(b1),
            predecessors: vec![],
            successors: vec![b1],
            exception_handler: None,
        });

        graph.blocks.push(BasicBlock {
            id: b1,
            operations: vec![],
            terminator: Terminator::Return(None),
            predecessors: vec![], // Missing b0!
            successors: vec![],
            exception_handler: None,
        });

        let result = verify_function_graph(&graph);
        assert!(matches!(result, Err(VerifierError::AsymmetricEdge { .. })));
    }
}
