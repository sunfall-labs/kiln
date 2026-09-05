//! Per-function summaries extracted from the SSA Value Graph.
//!
//! Step 1.5 of docs/architecture/fastest-native-plan.md:
//! Each compiled function produces a deterministic, compact summary: parameter and result types,
//! purity (pure-or-unknown), returned allocation/call sites, can-throw, and reads `arguments`.
//! Summaries are incorporated into module interfaces and project rollups, allowing body edits that
//! do not alter function summaries to achieve early cutoff.

use std::collections::{HashMap, HashSet};

use kiln_digest::Digest;
use kiln_ir::{FunctionProto, Instruction, Program};
use serde::{Deserialize, Serialize};

use crate::graph::{
    BasicBlock, BlockId, Effect, FunctionGraph, GraphOp, Proven, Terminator, ValueId,
    build_function_graph, proven_types,
};

/// Coarse lattice type representing a speculative or inferred type guess.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum TypeGuess {
    #[default]
    Unknown,
    Number,
    Int,
    Boolean,
    String,
    Object,
    Callable,
    Undefined,
    Null,
}

/// Compact per-function summary extracted from the SSA Value Graph / bytecode.
///
/// Functions with identical summaries do not invalidate callers or alter interface identity,
/// enabling early cutoff for function body edits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FunctionSummary {
    /// Inferred or guessed parameter types.
    pub parameter_types: Vec<TypeGuess>,
    /// Inferred or guessed return type across return sites.
    pub return_type: TypeGuess,
    /// Two-valued effect invariant: true for pure, false for unknown/side-effecting.
    pub pure_or_unknown: bool,
    /// Stable site IDs for returned allocations and call results.
    pub returned_sites: Vec<u32>,
    /// Whether this function may throw an exception.
    pub can_throw: bool,
    /// Whether this function accesses the legacy `arguments` object.
    pub reads_arguments: bool,
}

impl FunctionSummary {
    /// Convenience accessor for purity.
    pub fn is_pure(&self) -> bool {
        self.pure_or_unknown
    }

    /// Stable digest of this summary for hashing into interfaces and cache keys.
    pub fn digest(&self) -> Digest {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(b"kiln-function-summary-v1\0");
        encoded.extend_from_slice(&(self.parameter_types.len() as u64).to_le_bytes());
        for pt in &self.parameter_types {
            encoded.push(*pt as u8);
        }
        encoded.push(self.return_type as u8);
        encoded.push(if self.pure_or_unknown { 1 } else { 0 });
        encoded.extend_from_slice(&(self.returned_sites.len() as u64).to_le_bytes());
        for site in &self.returned_sites {
            encoded.extend_from_slice(&site.to_le_bytes());
        }
        encoded.push(if self.can_throw { 1 } else { 0 });
        encoded.push(if self.reads_arguments { 1 } else { 0 });
        Digest::hash(&encoded)
    }
}

/// Compute a collision-resistant aggregate digest over an ordered list of function summaries.
pub fn summaries_digest(summaries: &[FunctionSummary]) -> Digest {
    if summaries.is_empty() {
        return Digest::default();
    }
    let mut encoded = Vec::new();
    encoded.extend_from_slice(b"kiln-function-summaries-list-v1\0");
    encoded.extend_from_slice(&(summaries.len() as u64).to_le_bytes());
    for s in summaries {
        encoded.extend_from_slice(s.digest().as_bytes());
    }
    Digest::hash(&encoded)
}

/// Extract a `FunctionSummary` from an SSA Value Graph.
pub fn summarize_graph(graph: &FunctionGraph) -> FunctionSummary {
    let parameter_types = vec![TypeGuess::Unknown; graph.parameter_count as usize];

    // Compute reachable blocks starting from entry block (BlockId(0))
    let mut reachable = HashSet::new();
    if !graph.blocks.is_empty() {
        let mut worklist = vec![BlockId(0)];
        reachable.insert(BlockId(0));
        let block_map: HashMap<BlockId, &BasicBlock> =
            graph.blocks.iter().map(|b| (b.id, b)).collect();
        while let Some(bid) = worklist.pop() {
            if let Some(block) = block_map.get(&bid) {
                for &succ in &block.successors {
                    if reachable.insert(succ) {
                        worklist.push(succ);
                    }
                }
            }
        }
    }

    // Map each ValueId to its defining GraphOp and what is *proven* about its type. A summary
    // claims nothing it cannot prove.
    let proven = proven_types(graph);
    let mut val_map: HashMap<ValueId, (&GraphOp, TypeGuess)> =
        HashMap::with_capacity(graph.next_value_id as usize);
    for block in &graph.blocks {
        for (vid, op) in &block.operations {
            let guess = match proven.get(vid) {
                Some(Proven::Int | Proven::Number) => TypeGuess::Number,
                Some(Proven::Boolean) => TypeGuess::Boolean,
                Some(Proven::String) => TypeGuess::String,
                Some(Proven::Object) => TypeGuess::Object,
                Some(Proven::Callable) => TypeGuess::Callable,
                Some(Proven::Undefined) => TypeGuess::Undefined,
                Some(Proven::Null) => TypeGuess::Null,
                None => TypeGuess::Unknown,
            };
            val_map.insert(*vid, (op, guess));
        }
    }

    let mut return_types = Vec::new();
    let mut returned_sites = Vec::new();
    let mut returns_found = false;

    for block in graph.blocks.iter().filter(|b| reachable.contains(&b.id)) {
        if let Terminator::Return(maybe_val) = &block.terminator {
            returns_found = true;
            match maybe_val {
                None => {
                    return_types.push(TypeGuess::Undefined);
                }
                Some(val_id) => {
                    let mut visited = HashSet::new();
                    let ty = resolve_type(*val_id, &val_map, &mut visited);
                    return_types.push(ty);

                    let mut visited_sites = HashSet::new();
                    collect_sites(*val_id, &val_map, &mut visited_sites, &mut returned_sites);
                }
            }
        }
    }

    let return_type = if !returns_found || return_types.is_empty() {
        TypeGuess::Unknown
    } else {
        let first = return_types[0];
        if return_types.iter().all(|t| *t == first) {
            first
        } else {
            TypeGuess::Unknown
        }
    };

    returned_sites.sort_unstable();
    returned_sites.dedup();

    let reads_arguments = graph
        .blocks
        .iter()
        .filter(|b| reachable.contains(&b.id))
        .any(|b| {
            b.operations.iter().any(|(_, op)| {
                matches!(
                    op,
                    GraphOp::Bridge {
                        instruction: Instruction::LoadArguments,
                        ..
                    }
                )
            })
        });

    let mut can_throw = false;
    for block in graph.blocks.iter().filter(|b| reachable.contains(&b.id)) {
        if matches!(block.terminator, Terminator::Throw(_)) {
            can_throw = true;
            break;
        }
        if block.exception_handler.is_none() {
            for (_, op) in &block.operations {
                if op.effect() != Effect::Pure {
                    can_throw = true;
                    break;
                }
            }
        }
        if can_throw {
            break;
        }
    }

    let all_pure_ops = graph
        .blocks
        .iter()
        .filter(|b| reachable.contains(&b.id))
        .all(|b| {
            matches!(
                b.terminator,
                Terminator::Jump(_) | Terminator::Branch { .. } | Terminator::Return(_)
            ) && b.operations.iter().all(|(_, op)| {
                op.effect() == Effect::Pure || matches!(op, GraphOp::StoreLocal { .. })
            })
        });

    let pure_or_unknown = !can_throw && !reads_arguments && all_pure_ops;

    FunctionSummary {
        parameter_types,
        return_type,
        pure_or_unknown,
        returned_sites,
        can_throw,
        reads_arguments,
    }
}

fn resolve_type(
    val: ValueId,
    val_map: &HashMap<ValueId, (&GraphOp, TypeGuess)>,
    visited: &mut HashSet<ValueId>,
) -> TypeGuess {
    if !visited.insert(val) {
        return TypeGuess::Unknown;
    }
    if let Some((op, gtype)) = val_map.get(&val) {
        match op {
            GraphOp::Phi { inputs } => {
                let mut common_type = None;
                for (_, in_val) in inputs {
                    let in_type = resolve_type(*in_val, val_map, visited);
                    match common_type {
                        None => common_type = Some(in_type),
                        Some(prev) if prev == in_type => {}
                        _ => return TypeGuess::Unknown,
                    }
                }
                common_type.unwrap_or(TypeGuess::Unknown)
            }
            _ => *gtype,
        }
    } else {
        TypeGuess::Unknown
    }
}

fn collect_sites(
    val: ValueId,
    val_map: &HashMap<ValueId, (&GraphOp, TypeGuess)>,
    visited: &mut HashSet<ValueId>,
    sites: &mut Vec<u32>,
) {
    if !visited.insert(val) {
        return;
    }
    if let Some((op, _)) = val_map.get(&val) {
        match op {
            GraphOp::NewArray { site, .. }
            | GraphOp::MakeFunction { site, .. }
            | GraphOp::Call { site, .. }
            | GraphOp::CallMethod { site, .. }
            | GraphOp::GetProperty { site, .. }
            | GraphOp::GetComputed { site, .. }
            | GraphOp::SetProperty { site, .. }
            | GraphOp::SetPropertyStrict { site, .. }
            | GraphOp::SetComputed { site, .. }
            | GraphOp::SetComputedStrict { site, .. } => {
                sites.push(site.0);
            }
            GraphOp::Phi { inputs } => {
                for (_, in_val) in inputs {
                    collect_sites(*in_val, val_map, visited, sites);
                }
            }
            _ => {}
        }
    }
}

/// Extract a `FunctionSummary` from a function prototype and its containing program.
pub fn summarize_function(proto: &FunctionProto, program: &Program) -> FunctionSummary {
    match build_function_graph(proto, program) {
        Ok(graph) => summarize_graph(&graph),
        // A function the graph cannot represent has no facts to publish.
        Err(_) => FunctionSummary::default(),
    }
}

/// Extract summaries for all declared functions in a program.
pub fn summarize_program(program: &Program) -> Vec<FunctionSummary> {
    program
        .functions
        .iter()
        .map(|proto| summarize_function(proto, program))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_ir::{Constant, Instruction, Program, Span};

    fn empty_program() -> Program {
        Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: Vec::new(),
            code: Vec::new(),
            local_count: 0,
            functions: Vec::new(),
            source: "".into(),
        }
    }

    fn test_proto(
        name: &str,
        parameter_count: u32,
        local_count: u32,
        code: Vec<Instruction>,
    ) -> FunctionProto {
        FunctionProto {
            asynchronous: false,
            name: Some(name.to_string()),
            arrow: false,
            generator: false,
            async_generator: false,
            parameter_count,
            mapped_arguments: false,
            declared_length: parameter_count,
            local_count,
            prologue_end: 0,
            code,
            upvalues: Vec::new(),
            span: Span::default(),
        }
    }

    #[test]
    fn pure_scalar_constant_function_summary() {
        let mut program = empty_program();
        program.constants.push(Constant::Number(42.0f64.to_bits()));

        let proto = test_proto(
            "get_num",
            2,
            2,
            vec![Instruction::Const(0), Instruction::Return],
        );

        let summary = summarize_function(&proto, &program);
        assert_eq!(
            summary,
            FunctionSummary {
                parameter_types: vec![TypeGuess::Unknown, TypeGuess::Unknown],
                return_type: TypeGuess::Number,
                pure_or_unknown: true,
                returned_sites: Vec::new(),
                can_throw: false,
                reads_arguments: false,
            }
        );
        assert!(summary.is_pure());
    }

    #[test]
    fn new_array_allocation_records_returned_site() {
        let program = empty_program();
        let proto = test_proto(
            "alloc_arr",
            0,
            0,
            vec![Instruction::NewArray { count: 0 }, Instruction::Return],
        );

        let summary = summarize_function(&proto, &program);
        assert_eq!(summary.return_type, TypeGuess::Object);
        assert_eq!(summary.returned_sites, vec![0]);
        // NewArray is not pure (allocates heap state)
        assert!(!summary.pure_or_unknown);
    }

    #[test]
    fn throwing_function_marked_can_throw() {
        let mut program = empty_program();
        program.constants.push(Constant::String("err".to_string()));

        let proto = test_proto(
            "fail",
            0,
            0,
            vec![Instruction::Const(0), Instruction::Throw],
        );

        let summary = summarize_function(&proto, &program);
        assert!(summary.can_throw);
        assert!(!summary.pure_or_unknown);
    }

    #[test]
    fn function_reading_arguments_detected() {
        let program = empty_program();
        let proto = test_proto(
            "has_args",
            0,
            0,
            vec![Instruction::LoadArguments, Instruction::Return],
        );

        let summary = summarize_function(&proto, &program);
        assert!(summary.reads_arguments);
        assert!(!summary.pure_or_unknown);
    }

    #[test]
    fn summary_digest_is_sensitive_to_summary_changes() {
        let s1 = FunctionSummary {
            parameter_types: vec![TypeGuess::Unknown],
            return_type: TypeGuess::Number,
            pure_or_unknown: true,
            returned_sites: vec![],
            can_throw: false,
            reads_arguments: false,
        };
        let mut s2 = s1.clone();
        assert_eq!(s1.digest(), s2.digest());

        // Changing return type changes digest
        s2.return_type = TypeGuess::String;
        assert_ne!(s1.digest(), s2.digest());

        // Changing can_throw changes digest
        let mut s3 = s1.clone();
        s3.can_throw = true;
        assert_ne!(s1.digest(), s3.digest());

        // Changing purity changes digest
        let mut s4 = s1.clone();
        s4.pure_or_unknown = false;
        assert_ne!(s1.digest(), s4.digest());

        // Changing returned_sites changes digest
        let mut s5 = s1.clone();
        s5.returned_sites = vec![1];
        assert_ne!(s1.digest(), s5.digest());
    }
}
