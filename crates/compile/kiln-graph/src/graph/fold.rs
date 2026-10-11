use super::*;

fn to_int32(val: f64) -> i32 {
    if !val.is_finite() || val == 0.0 {
        0
    } else {
        let rem = val.trunc() % 4294967296.0;
        rem as i64 as i32
    }
}

fn to_uint32(val: f64) -> u32 {
    to_int32(val) as u32
}

fn constant_truthy(c: &Constant) -> bool {
    match c {
        Constant::Undefined | Constant::Null => false,
        Constant::Boolean(b) => *b,
        Constant::Number(bits) => {
            let f = f64::from_bits(*bits);
            f != 0.0 && !f.is_nan()
        }
        Constant::String(s) => !s.is_empty(),
        Constant::BigInt(s) => s != "0" && !s.is_empty(),
        Constant::WellKnownSymbol(_) => true,
    }
}

fn fold_unary(op: UnaryOp, c: &Constant) -> Option<Constant> {
    match op {
        UnaryOp::Not => Some(Constant::Boolean(!constant_truthy(c))),
        UnaryOp::Void => Some(Constant::Undefined),
        UnaryOp::Negate => match c {
            Constant::Number(bits) => {
                let f = -f64::from_bits(*bits);
                Some(Constant::Number(f.to_bits()))
            }
            _ => None,
        },
        UnaryOp::Plus => match c {
            Constant::Number(bits) => Some(Constant::Number(*bits)),
            Constant::Boolean(b) => {
                Some(Constant::Number(if *b { 1.0f64 } else { 0.0f64 }.to_bits()))
            }
            Constant::Null => Some(Constant::Number(0.0f64.to_bits())),
            Constant::Undefined => Some(Constant::Number(f64::NAN.to_bits())),
            _ => None,
        },
        UnaryOp::BitwiseNot => match c {
            Constant::Number(bits) => {
                let n = to_int32(f64::from_bits(*bits));
                Some(Constant::Number(((!n) as f64).to_bits()))
            }
            Constant::Boolean(b) => {
                let n = if *b { 1i32 } else { 0i32 };
                Some(Constant::Number(((!n) as f64).to_bits()))
            }
            Constant::Null | Constant::Undefined => Some(Constant::Number((-1.0f64).to_bits())),
            _ => None,
        },
        UnaryOp::ToNumeric => match c {
            Constant::Number(bits) => Some(Constant::Number(*bits)),
            _ => None,
        },
        _ => None,
    }
}

fn fold_binary(op: BinaryOp, left: &Constant, right: &Constant) -> Option<Constant> {
    match op {
        BinaryOp::Add => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = f64::from_bits(*a) + f64::from_bits(*b);
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::Subtract => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = f64::from_bits(*a) - f64::from_bits(*b);
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::Multiply => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = f64::from_bits(*a) * f64::from_bits(*b);
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::Divide => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = f64::from_bits(*a) / f64::from_bits(*b);
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::Remainder => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = f64::from_bits(*a) % f64::from_bits(*b);
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::Exponentiate => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = f64::from_bits(*a).powf(f64::from_bits(*b));
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::BitwiseAnd => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = (to_int32(f64::from_bits(*a)) & to_int32(f64::from_bits(*b))) as f64;
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::BitwiseOr => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = (to_int32(f64::from_bits(*a)) | to_int32(f64::from_bits(*b))) as f64;
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::BitwiseXor => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = (to_int32(f64::from_bits(*a)) ^ to_int32(f64::from_bits(*b))) as f64;
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::ShiftLeft => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let shift = to_uint32(f64::from_bits(*b)) & 0x1f;
                let val = (to_int32(f64::from_bits(*a)) << shift) as f64;
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::ShiftRight => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let shift = to_uint32(f64::from_bits(*b)) & 0x1f;
                let val = (to_int32(f64::from_bits(*a)) >> shift) as f64;
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::UnsignedShiftRight => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let shift = to_uint32(f64::from_bits(*b)) & 0x1f;
                let val = (to_uint32(f64::from_bits(*a)) >> shift) as f64;
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::StrictEqual => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let fa = f64::from_bits(*a);
                let fb = f64::from_bits(*b);
                Some(Constant::Boolean(fa == fb))
            }
            (Constant::Boolean(a), Constant::Boolean(b)) => Some(Constant::Boolean(a == b)),
            (Constant::Null, Constant::Null) => Some(Constant::Boolean(true)),
            (Constant::Undefined, Constant::Undefined) => Some(Constant::Boolean(true)),
            (Constant::Null, Constant::Undefined) | (Constant::Undefined, Constant::Null) => {
                Some(Constant::Boolean(false))
            }
            _ => None,
        },
        BinaryOp::StrictNotEqual => match fold_binary(BinaryOp::StrictEqual, left, right) {
            Some(Constant::Boolean(b)) => Some(Constant::Boolean(!b)),
            _ => None,
        },
        BinaryOp::LessThan => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let fa = f64::from_bits(*a);
                let fb = f64::from_bits(*b);
                Some(Constant::Boolean(fa < fb))
            }
            _ => None,
        },
        BinaryOp::LessThanOrEqual => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let fa = f64::from_bits(*a);
                let fb = f64::from_bits(*b);
                Some(Constant::Boolean(fa <= fb))
            }
            _ => None,
        },
        BinaryOp::GreaterThan => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let fa = f64::from_bits(*a);
                let fb = f64::from_bits(*b);
                Some(Constant::Boolean(fa > fb))
            }
            _ => None,
        },
        BinaryOp::GreaterThanOrEqual => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let fa = f64::from_bits(*a);
                let fb = f64::from_bits(*b);
                Some(Constant::Boolean(fa >= fb))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Constant folding pass over the SSA Value Graph.
/// Evaluates pure operations with known constant operands and replaces them with `GraphOp::Const`.
pub fn fold_constants(graph: &mut FunctionGraph) {
    let mut constants: HashMap<ValueId, Constant> = HashMap::new();
    loop {
        let mut changed = false;
        for block in &mut graph.blocks {
            for (vid, op) in &mut block.operations {
                match op {
                    GraphOp::Const { constant, .. } => {
                        if !constants.contains_key(vid) {
                            constants.insert(*vid, constant.clone());
                        }
                    }
                    GraphOp::Unary { op: u_op, value } => {
                        if let Some(c) = constants.get(value) {
                            if let Some(folded) = fold_unary(*u_op, c) {
                                *op = GraphOp::Const {
                                    index: 0,
                                    constant: folded.clone(),
                                };
                                constants.insert(*vid, folded);
                                changed = true;
                            }
                        }
                    }
                    GraphOp::Binary {
                        op: b_op,
                        left,
                        right,
                    } => {
                        if let (Some(cl), Some(cr)) = (constants.get(left), constants.get(right)) {
                            if let Some(folded) = fold_binary(*b_op, cl, cr) {
                                *op = GraphOp::Const {
                                    index: 0,
                                    constant: folded.clone(),
                                };
                                constants.insert(*vid, folded);
                                changed = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Normalize returns of constant `undefined` to `Terminator::Return(None)`, allowing dead
/// constant definitions to be eliminated and direct undefined return pathways to be emitted.
pub fn fold_undefined_returns(graph: &mut FunctionGraph) {
    let mut undefined_values = HashSet::new();
    for block in &graph.blocks {
        for (vid, op) in &block.operations {
            if let GraphOp::Const {
                constant: Constant::Undefined,
                ..
            } = op
            {
                undefined_values.insert(*vid);
            }
        }
    }
    if undefined_values.is_empty() {
        return;
    }
    for block in &mut graph.blocks {
        if let Terminator::Return(Some(vid)) = &block.terminator
            && undefined_values.contains(vid)
        {
            block.terminator = Terminator::Return(None);
        }
    }
}

/// Prune unreachable basic blocks from the CFG, updating predecessors and Phi inputs in surviving blocks.
pub fn prune_unreachable_blocks(graph: &mut FunctionGraph) {
    if graph.blocks.is_empty() {
        return;
    }

    let mut reachable = HashSet::new();
    let mut queue = std::collections::VecDeque::new();

    reachable.insert(graph.entry_block);
    queue.push_back(graph.entry_block);

    if let Some(seam) = graph.seam_block {
        if reachable.insert(seam) {
            queue.push_back(seam);
        }
    }

    let block_map: HashMap<BlockId, &BasicBlock> = graph.blocks.iter().map(|b| (b.id, b)).collect();

    while let Some(bid) = queue.pop_front() {
        if let Some(b) = block_map.get(&bid) {
            for &succ in &b.successors {
                if reachable.insert(succ) {
                    queue.push_back(succ);
                }
            }
            if let Some(handler) = b.exception_handler {
                if reachable.insert(handler) {
                    queue.push_back(handler);
                }
            }
            if let Terminator::Suspend {
                resume, delegate, ..
            } = &b.terminator
            {
                if reachable.insert(*resume) {
                    queue.push_back(*resume);
                }
                if let Some((_, exit, _)) = delegate
                    && reachable.insert(*exit)
                {
                    queue.push_back(*exit);
                }
            }
        }
    }

    if reachable.len() == graph.blocks.len() {
        return;
    }

    // Filter predecessors and Phi inputs in surviving blocks
    for block in &mut graph.blocks {
        if reachable.contains(&block.id) {
            block.predecessors.retain(|p| reachable.contains(p));
            for (_, op) in &mut block.operations {
                if let GraphOp::Phi { inputs } = op {
                    inputs.retain(|(p, _)| reachable.contains(p));
                }
            }
        }
    }

    // Prune unreachable blocks
    graph.blocks.retain(|b| reachable.contains(&b.id));
}

/// Fold conditional branches whose condition is statically known to a constant truthy/falsy value
/// or whose true and false targets are identical into unconditional jumps, and prune newly unreachable blocks.
pub fn fold_conditional_branches(graph: &mut FunctionGraph) -> bool {
    let mut constants: HashMap<ValueId, Constant> = HashMap::new();
    for block in &graph.blocks {
        for (vid, op) in &block.operations {
            if let GraphOp::Const { constant, .. } = op {
                constants.insert(*vid, constant.clone());
            }
        }
    }

    let mut changed = false;
    let mut eliminated_edges: Vec<(BlockId, BlockId)> = Vec::new();

    for block in &mut graph.blocks {
        let replacement = match &block.terminator {
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            } => {
                if true_block == false_block {
                    Some((*true_block, None))
                } else if let Some(c) = constants.get(condition) {
                    if constant_truthy(c) {
                        Some((*true_block, Some(*false_block)))
                    } else {
                        Some((*false_block, Some(*true_block)))
                    }
                } else {
                    None
                }
            }
            Terminator::BranchNotNullish {
                condition,
                true_block,
                false_block,
            } => {
                if true_block == false_block {
                    Some((*true_block, None))
                } else if let Some(c) = constants.get(condition) {
                    let nullish = matches!(c, Constant::Null | Constant::Undefined);
                    if !nullish {
                        Some((*true_block, Some(*false_block)))
                    } else {
                        Some((*false_block, Some(*true_block)))
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some((target, eliminated_target)) = replacement {
            block.terminator = Terminator::Jump(target);
            let mut successors = vec![target];
            if let Some(handler) = block.exception_handler
                && !successors.contains(&handler)
            {
                successors.push(handler);
            }
            block.successors = successors;
            if let Some(elim) = eliminated_target {
                eliminated_edges.push((block.id, elim));
            }
            changed = true;
        }
    }

    if !changed {
        return false;
    }

    // Update predecessors and Phis in eliminated target blocks
    for (from_block, to_block) in eliminated_edges {
        if let Some(from_b) = graph.blocks.iter().find(|b| b.id == from_block)
            && from_b.successors.contains(&to_block)
        {
            continue;
        }
        if let Some(b) = graph.blocks.iter_mut().find(|b| b.id == to_block) {
            b.predecessors.retain(|&p| p != from_block);
            for (_, op) in &mut b.operations {
                if let GraphOp::Phi { inputs } = op {
                    inputs.retain(|(p, _)| *p != from_block);
                }
            }
        }
    }

    prune_unreachable_blocks(graph);
    true
}

/// Merge sequential basic blocks where block A unconditionally jumps to block B,
/// B has A as its only predecessor, exception handling scopes match, and B is not
/// a protected entry, seam, exception handler, or coroutine resume target.
pub fn merge_basic_blocks(graph: &mut FunctionGraph) -> bool {
    if graph.blocks.len() < 2 {
        return false;
    }

    let mut protected_targets = HashSet::new();
    protected_targets.insert(graph.entry_block);
    if let Some(seam) = graph.seam_block {
        protected_targets.insert(seam);
    }
    for block in &graph.blocks {
        if let Some(h) = block.exception_handler {
            protected_targets.insert(h);
        }
        if let Terminator::Suspend {
            resume, delegate, ..
        } = &block.terminator
        {
            protected_targets.insert(*resume);
            if let Some((_, exit, _)) = delegate {
                protected_targets.insert(*exit);
            }
        }
    }

    let mut changed = false;
    loop {
        let mut merge_candidate: Option<(BlockId, BlockId)> = None;

        for block in &graph.blocks {
            if let Terminator::Jump(target) = block.terminator
                && target != block.id
                && !protected_targets.contains(&target)
                && let Some(target_block) = graph.blocks.iter().find(|b| b.id == target)
                && target_block.predecessors.len() == 1
                && target_block.predecessors[0] == block.id
                && block.exception_handler == target_block.exception_handler
                && !matches!(target_block.terminator, Terminator::Suspend { .. })
                && !target_block
                    .operations
                    .iter()
                    .any(|(_, op)| matches!(op, GraphOp::Phi { .. }))
            {
                merge_candidate = Some((block.id, target));
                break;
            }
        }

        let Some((a_id, b_id)) = merge_candidate else {
            break;
        };

        let b_idx = graph.blocks.iter().position(|b| b.id == b_id).unwrap();
        let b_block = graph.blocks.remove(b_idx);

        let a_block = graph.blocks.iter_mut().find(|b| b.id == a_id).unwrap();
        a_block.operations.extend(b_block.operations);
        a_block.terminator = b_block.terminator;
        a_block.successors = b_block.successors.clone();

        for succ_id in &b_block.successors {
            if let Some(succ_block) = graph.blocks.iter_mut().find(|b| b.id == *succ_id) {
                if succ_block.predecessors.contains(&a_id) {
                    succ_block.predecessors.retain(|&p| p != b_id);
                } else {
                    for p in &mut succ_block.predecessors {
                        if *p == b_id {
                            *p = a_id;
                        }
                    }
                }

                for (_, op) in &mut succ_block.operations {
                    if let GraphOp::Phi { inputs } = op {
                        for (p, _) in inputs {
                            if *p == b_id {
                                *p = a_id;
                            }
                        }
                    }
                }
            }
        }

        changed = true;
    }

    changed
}

/// Alias for dead code elimination preserving existing test entry points.
pub fn remove_dead_phis(graph: &mut FunctionGraph) {
    eliminate_dead_code(graph);
}

/// Remove dead operations and dead phi chains with no consumers (ADR 0061).
///
/// An operation is inherently live if it has observable side-effects (throwing, calling user code,
/// allocating, or writing heap/host), or if it is a local variable store (`StoreLocal`) whose slot
/// may be accessed in a frame/array, or a phi in a generator resume block. Pure operations with no
/// consumers are soundly eliminated.
pub fn eliminate_dead_code(graph: &mut FunctionGraph) {
    let mut live = HashSet::new();
    let mut resumes = HashSet::new();
    for block in &graph.blocks {
        match &block.terminator {
            Terminator::Return(Some(value))
            | Terminator::Throw(value)
            | Terminator::Branch {
                condition: value, ..
            }
            | Terminator::BranchNotNullish {
                condition: value, ..
            } => {
                live.insert(*value);
            }
            Terminator::Suspend {
                value,
                spilled,
                resume,
                delegate,
                delegate_wrapped,
                ..
            } => {
                live.insert(*value);
                if let Some((iter, exit, _)) = delegate {
                    live.insert(*iter);
                    resumes.insert(*exit);
                }
                live.extend(delegate_wrapped.iter().copied());
                live.extend(spilled.iter().copied());
                resumes.insert(*resume);
            }
            _ => {}
        }
    }

    let entry_block = graph.entry_block;
    let proven_numbers = proven_numbers(graph);
    let is_inherently_live = |op: &GraphOp, block_id: BlockId| -> bool {
        if matches!(op, GraphOp::Phi { .. }) {
            return resumes.contains(&block_id);
        }
        if matches!(op, GraphOp::StoreLocal { .. }) {
            return true;
        }
        if matches!(op, GraphOp::LoadLocal(_)) && block_id == entry_block {
            return true;
        }
        let effects = op.effects_with_proven_numbers(&proven_numbers);
        effects.intersects(
            Effects::MAY_THROW
                | Effects::CALLS_USER_CODE
                | Effects::ALLOCATES
                | Effects::WRITES_HEAP
                | Effects::WRITES_HOST
                | Effects::MAY_SUSPEND,
        )
    };

    let mut def_operands: HashMap<ValueId, Vec<ValueId>> = HashMap::new();
    for block in &graph.blocks {
        for (value, op) in &block.operations {
            if is_inherently_live(op, block.id) {
                live.insert(*value);
                live.extend(op.operands());
            } else {
                def_operands.insert(*value, op.operands());
            }
        }
    }

    let mut pending: Vec<ValueId> = live.iter().copied().collect();
    while let Some(value) = pending.pop() {
        if let Some(operands) = def_operands.get(&value) {
            for operand in operands {
                if live.insert(*operand) {
                    pending.push(*operand);
                }
            }
        }
    }

    for block in &mut graph.blocks {
        block
            .operations
            .retain(|(value, op)| is_inherently_live(op, block.id) || live.contains(value));
    }
}
