//! Shared argument-object facts over a program and its verified function graphs.

use crate::{FunctionGraph, GraphOp, Terminator, ValueId};
use kiln_ir::{Constant, FunctionProto, Instruction, Program, UnaryOp};
use std::collections::{HashMap, HashSet, VecDeque};

/// The SSA `LoadArguments` results whose only use is the own `.length` read.
#[derive(Clone, Debug, Default)]
pub struct ArgumentsLengthFact {
    /// The verified graph values corresponding to argc-only `.length` reads.
    pub values: HashSet<ValueId>,
    /// True when no reachable load exists, or every reachable load was proven to be `.length`.
    /// False means the proof was rejected and the arguments object remains observable.
    pub reads_elided: bool,
}

/// One independently proven readonly arguments member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgumentsMember {
    Length,
    Index(u32),
}

/// Fast-path candidates for single-use static member reads. This fact never makes a function
/// frame elidable: the emitted guard checks whether the real object already exists.
#[derive(Clone, Debug, Default)]
pub struct ArgumentsMemberFact {
    /// GetProperty result values that may use the guarded member path.
    pub members: HashMap<ValueId, ArgumentsMember>,
    /// LoadArguments values whose complete use set consists of guarded member reads.
    pub elidable_arguments: HashSet<ValueId>,
}

/// Per-arrow lexical-arguments capture requirements, computed once for one exact Program.
///
/// `arrow_captures_arguments[i]` means arrow `i` directly reads the inherited arguments or makes
/// an arrow descendant that does. Non-arrow protos are always false: they own the arguments for
/// their own invocation. The bytecode scan is intentionally conservative: even unreachable
/// `LoadArguments` and `DirectEval` instructions keep capture.
///
/// These facts do not own the source `Program`. A caller that reuses them must keep the exact
/// Program alive and validate its identity (for example, by upgrading a weak Program handle)
/// until analysis using the facts is complete. Pointer identity alone is not a lifetime token.
#[derive(Clone, Debug)]
pub struct ArrowArgumentsCaptureFacts {
    program_identity: usize,
    arrow_captures_arguments: std::sync::Arc<[bool]>,
}

impl ArrowArgumentsCaptureFacts {
    pub fn for_program(program: &Program) -> Self {
        let count = program.functions.len();
        let mut parents = vec![Vec::<usize>::new(); count];
        let mut needs = vec![false; count];
        let mut pending = VecDeque::new();

        for (parent, proto) in program.functions.iter().enumerate() {
            for instruction in &proto.code {
                let Instruction::MakeFunction(child) = instruction else {
                    continue;
                };
                let child = *child as usize;
                if proto.arrow
                    && program
                        .functions
                        .get(child)
                        .is_some_and(|child_proto| child_proto.arrow)
                {
                    parents[child].push(parent);
                }
            }
            if proto.arrow
                && proto.code.iter().any(|instruction| {
                    matches!(
                        instruction,
                        Instruction::LoadArguments | Instruction::DirectEval(_)
                    )
                })
            {
                needs[parent] = true;
                pending.push_back(parent);
            }
        }

        // Reverse-edge worklist handles repeated references and malformed cyclic function
        // tables without recursion or an O(proto_count * instruction_count) fixed-point scan.
        while let Some(child) = pending.pop_front() {
            for &parent in &parents[child] {
                if !needs[parent] {
                    needs[parent] = true;
                    pending.push_back(parent);
                }
            }
        }

        Self {
            program_identity: program as *const Program as usize,
            arrow_captures_arguments: needs.into(),
        }
    }

    pub fn is_for_program(&self, program: &Program) -> bool {
        self.program_identity == program as *const Program as usize
            && self.arrow_captures_arguments.len() == program.functions.len()
    }

    pub fn arrow_captures_arguments(&self, function: u32) -> bool {
        self.arrow_captures_arguments
            .get(function as usize)
            .copied()
            .unwrap_or(false)
    }

    pub fn arrow_capture_facts(&self) -> &[bool] {
        &self.arrow_captures_arguments
    }
}

/// Program-scoped arguments analysis. The reusable capture facts must belong to the same exact
/// Program instance; this matters because separate module phases may have different function
/// tables even when they originated from one authored module.
pub struct ArgumentsAnalysis<'program> {
    program: &'program Program,
    arrow_captures: ArrowArgumentsCaptureFacts,
}

impl<'program> ArgumentsAnalysis<'program> {
    pub fn new(program: &'program Program) -> Self {
        let arrow_captures = ArrowArgumentsCaptureFacts::for_program(program);
        Self {
            program,
            arrow_captures,
        }
    }

    /// Reuse facts from a weak identity-validated Program cache without repeating the
    /// program-wide worklist. Returns `None` if they were computed for another Program instance.
    pub fn with_arrow_capture_facts(
        program: &'program Program,
        arrow_captures: &ArrowArgumentsCaptureFacts,
    ) -> Option<Self> {
        if !arrow_captures.is_for_program(program) {
            return None;
        }
        Some(Self {
            program,
            arrow_captures: arrow_captures.clone(),
        })
    }

    pub fn arrow_capture_facts(&self) -> &ArrowArgumentsCaptureFacts {
        &self.arrow_captures
    }

    pub fn argument_length_fact(
        &self,
        function: usize,
        graph: &FunctionGraph,
    ) -> ArgumentsLengthFact {
        let Some(proto) = self.program.functions.get(function) else {
            return ArgumentsLengthFact::default();
        };
        self.argument_length_fact_for_member(graph, proto)
    }

    /// Prove each own `arguments.length` or canonical-index read independently. The result is
    /// separate from the argc-only length proof and never changes frame-publication decisions.
    pub fn argument_member_fact(
        &self,
        function: usize,
        graph: &FunctionGraph,
    ) -> ArgumentsMemberFact {
        let Some(proto) = self.program.functions.get(function) else {
            return ArgumentsMemberFact::default();
        };
        if proto.arrow
            || proto.generator
            || proto.asynchronous
            || proto.async_generator
            || proto.mapped_arguments
            || proto
                .code
                .iter()
                .any(|i| matches!(i, Instruction::DirectEval(_)))
        {
            return ArgumentsMemberFact::default();
        }
        if !proto
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadArguments))
        {
            return ArgumentsMemberFact::default();
        }
        let mut loads = HashSet::new();
        let mut uses = HashMap::<ValueId, Vec<&GraphOp>>::new();
        let mut terminal = HashSet::new();
        let mut constant_numbers = HashMap::<ValueId, f64>::new();
        let mut property_reads = HashMap::<ValueId, Vec<(ValueId, ArgumentsMember)>>::new();
        for block in &graph.blocks {
            for (value, op) in &block.operations {
                match op {
                    GraphOp::Const {
                        constant: Constant::Number(bits),
                        ..
                    } => {
                        constant_numbers.insert(*value, f64::from_bits(*bits));
                    }
                    GraphOp::Unary {
                        op: UnaryOp::Negate,
                        value: operand,
                    } => {
                        if let Some(number) = constant_numbers.get(operand) {
                            constant_numbers.insert(*value, -*number);
                        }
                    }
                    _ => {}
                }
            }
        }
        for block in &graph.blocks {
            for (value, op) in &block.operations {
                if matches!(
                    op,
                    GraphOp::Bridge {
                        instruction: Instruction::LoadArguments,
                        pushes: 1,
                        ..
                    }
                ) {
                    loads.insert(*value);
                }
                for input in op.operands() {
                    uses.entry(input).or_default().push(op);
                }
                if let GraphOp::GetProperty { object, name, .. } = op
                    && let Some(Constant::String(units)) =
                        self.program.constants.get(*name as usize)
                    && let Ok(text) = units.to_scalar_string()
                {
                    let member = if text == "length" {
                        Some(ArgumentsMember::Length)
                    } else {
                        canonical_argument_index(&text).map(ArgumentsMember::Index)
                    };
                    if let Some(member) = member {
                        property_reads
                            .entry(*object)
                            .or_default()
                            .push((*value, member));
                    }
                }
                if let GraphOp::GetComputed { object, key, .. } = op
                    && let Some(number) = constant_numbers.get(key)
                    && let Some(index) = canonical_argument_number(*number)
                {
                    property_reads
                        .entry(*object)
                        .or_default()
                        .push((*value, ArgumentsMember::Index(index)));
                }
            }
            match &block.terminator {
                Terminator::Branch { condition, .. }
                | Terminator::BranchNotNullish { condition, .. }
                | Terminator::Throw(condition)
                | Terminator::Return(Some(condition)) => {
                    terminal.insert(*condition);
                }
                Terminator::Suspend {
                    value,
                    spilled,
                    delegate,
                    delegate_wrapped,
                    ..
                } => {
                    terminal.insert(*value);
                    terminal.extend(spilled.iter().copied());
                    if let Some((it, _, _)) = delegate {
                        terminal.insert(*it);
                    }
                    terminal.extend(delegate_wrapped.iter().copied());
                }
                _ => {}
            }
        }
        if loads.is_empty() {
            return ArgumentsMemberFact::default();
        }
        let mut members = HashMap::new();
        let mut elidable_arguments = HashSet::new();
        for value in &loads {
            let Some(reads) = property_reads.get(value) else {
                continue;
            };
            let Some(value_uses) = uses.get(value) else {
                continue;
            };
            let all_member_reads = !terminal.contains(value)
                && value_uses.len() == reads.len()
                && value_uses.iter().all(|operation| match operation {
                    GraphOp::GetProperty { object, .. } | GraphOp::GetComputed { object, .. } => {
                        object == value
                    }
                    _ => false,
                });
            if all_member_reads {
                members.extend(reads.iter().copied());
                elidable_arguments.insert(*value);
            }
        }
        ArgumentsMemberFact {
            members,
            elidable_arguments,
        }
    }

    /// Standalone fallback for callers that do not have a function index. The proto must be an
    /// element of this analysis's exact Program; otherwise its constant and child indexes could
    /// be interpreted against the wrong program. The indexed planner path uses
    /// [`Self::argument_length_fact`] and avoids this linear membership check.
    pub fn argument_length_fact_for(
        &self,
        graph: &FunctionGraph,
        proto: &FunctionProto,
    ) -> ArgumentsLengthFact {
        if !self
            .program
            .functions
            .iter()
            .any(|member| std::ptr::eq(member, proto))
        {
            return ArgumentsLengthFact::default();
        }
        self.argument_length_fact_for_member(graph, proto)
    }

    fn argument_length_fact_for_member(
        &self,
        graph: &FunctionGraph,
        proto: &FunctionProto,
    ) -> ArgumentsLengthFact {
        if !proto
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadArguments))
        {
            return ArgumentsLengthFact {
                values: HashSet::new(),
                reads_elided: true,
            };
        }
        if proto.arrow || proto.generator || proto.asynchronous || proto.async_generator {
            return ArgumentsLengthFact::default();
        }
        if proto
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::DirectEval(_)))
        {
            return ArgumentsLengthFact::default();
        }
        if proto.code.iter().any(|instruction| {
            let Instruction::MakeFunction(child) = instruction else {
                return false;
            };
            self.program
                .functions
                .get(*child as usize)
                .is_some_and(|child_proto| {
                    child_proto.arrow && self.arrow_captures.arrow_captures_arguments(*child)
                })
        }) {
            return ArgumentsLengthFact::default();
        }

        let mut loads = HashSet::new();
        let mut uses = HashMap::<ValueId, Vec<&GraphOp>>::new();
        let mut terminator_uses = HashSet::new();
        for block in &graph.blocks {
            for (value, operation) in &block.operations {
                if matches!(
                    operation,
                    GraphOp::Bridge {
                        instruction: Instruction::LoadArguments,
                        pushes: 1,
                        ..
                    }
                ) {
                    loads.insert(*value);
                }
                for input in operation.operands() {
                    uses.entry(input).or_default().push(operation);
                }
            }
            match &block.terminator {
                Terminator::Branch { condition, .. }
                | Terminator::BranchNotNullish { condition, .. }
                | Terminator::Throw(condition)
                | Terminator::Return(Some(condition)) => {
                    terminator_uses.insert(*condition);
                }
                Terminator::Suspend {
                    value,
                    spilled,
                    delegate,
                    delegate_wrapped,
                    ..
                } => {
                    terminator_uses.insert(*value);
                    terminator_uses.extend(spilled.iter().copied());
                    if let Some((iterator, _, _)) = delegate {
                        terminator_uses.insert(*iterator);
                    }
                    terminator_uses.extend(delegate_wrapped.iter().copied());
                }
                Terminator::Jump(_) | Terminator::Return(None) | Terminator::Unreachable => {}
            }
        }
        if loads.is_empty() {
            return ArgumentsLengthFact {
                values: loads,
                reads_elided: true,
            };
        }

        let mut optimized = HashSet::new();
        for value in loads.iter().copied() {
            if terminator_uses.contains(&value) {
                return ArgumentsLengthFact::default();
            }
            let Some(value_uses) = uses.get(&value) else {
                return ArgumentsLengthFact::default();
            };
            if value_uses.len() != 1 {
                return ArgumentsLengthFact::default();
            }
            let GraphOp::GetProperty { object, name, .. } = value_uses[0] else {
                return ArgumentsLengthFact::default();
            };
            let is_length = matches!(
                self.program.constants.get(*name as usize),
                Some(Constant::String(units))
                    if units.to_scalar_string().as_deref() == Ok("length")
            );
            if *object != value || !is_length {
                return ArgumentsLengthFact::default();
            }
            optimized.insert(value);
        }
        ArgumentsLengthFact {
            reads_elided: optimized.len() == loads.len(),
            values: optimized,
        }
    }
}

fn canonical_argument_index(text: &str) -> Option<u32> {
    if text.is_empty()
        || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let index = text.parse::<u32>().ok()?;
    (index != u32::MAX).then_some(index)
}

fn canonical_argument_number(number: f64) -> Option<u32> {
    if !number.is_finite() || number.fract() != 0.0 || number < 0.0 || number >= u32::MAX as f64 {
        return None;
    }
    Some(number as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_ir::{EvalNewTarget, EvalSite, Span, Upvalue};

    fn proto(arrow: bool, code: Vec<Instruction>) -> FunctionProto {
        FunctionProto {
            asynchronous: false,
            name: None,
            inferred_name: None,
            arrow,
            generator: false,
            async_generator: false,
            is_method: false,
            is_class: false,
            parameter_count: 0,
            mapped_arguments: false,
            declared_length: 0,
            local_count: 0,
            prologue_end: 0,
            code,
            upvalues: Vec::<Upvalue>::new(),
            span: Span::default(),
            type_annotations: None,
        }
    }

    fn program(functions: Vec<FunctionProto>) -> Program {
        Program {
            functions,
            ..Program::default()
        }
    }

    #[test]
    fn nested_arrow_argument_capture_propagates_only_through_arrows() {
        let program = program(vec![
            proto(true, vec![Instruction::MakeFunction(1)]),
            proto(true, vec![Instruction::MakeFunction(2)]),
            proto(true, vec![Instruction::LoadArguments]),
            // A regular function owns its arguments; its arrow child captures that function's
            // arguments, never an enclosing arrow's lexical arguments.
            proto(true, vec![Instruction::MakeFunction(4)]),
            proto(false, vec![Instruction::LoadArguments]),
            proto(false, vec![Instruction::MakeFunction(6)]),
            proto(true, vec![Instruction::LoadArguments]),
        ]);
        let facts = ArrowArgumentsCaptureFacts::for_program(&program);
        assert!(facts.arrow_captures_arguments(0));
        assert!(facts.arrow_captures_arguments(1));
        assert!(facts.arrow_captures_arguments(2));
        assert!(!facts.arrow_captures_arguments(3));
        assert!(!facts.arrow_captures_arguments(4));
        assert!(!facts.arrow_captures_arguments(5));
        assert!(facts.arrow_captures_arguments(6));
    }

    #[test]
    fn direct_eval_is_conservatively_an_arrow_arguments_capture() {
        let program = program(vec![proto(
            true,
            vec![Instruction::DirectEval(Box::new(EvalSite {
                module_id: "arguments-proof".into(),
                source_offset: 0,
                bindings: Vec::new(),
                with_objects: Vec::new(),
                strict: false,
                argc: Some(0),
                global_var_scope: false,
                home: None,
                new_target: EvalNewTarget::Forbidden,
                private_scopes: Vec::new(),
                blocked_var_names: Vec::new(),
            }))],
        )]);
        let facts = ArrowArgumentsCaptureFacts::for_program(&program);
        assert!(facts.arrow_captures_arguments(0));
    }

    #[test]
    fn capture_worklist_handles_deep_and_cyclic_function_tables() {
        const DEPTH: usize = 10_000;
        let mut functions = (0..DEPTH)
            .map(|index| {
                if index + 1 < DEPTH {
                    proto(true, vec![Instruction::MakeFunction((index + 1) as u32)])
                } else {
                    proto(true, vec![Instruction::LoadArguments])
                }
            })
            .collect::<Vec<_>>();
        // Valid bytecode tables are acyclic, but malformed tables must not recurse forever.
        functions[DEPTH - 1].code.push(Instruction::MakeFunction(0));
        let program = program(functions);
        let facts = ArrowArgumentsCaptureFacts::for_program(&program);
        assert!(facts.arrow_captures_arguments(0));
        assert!(facts.arrow_captures_arguments((DEPTH - 1) as u32));
    }

    #[test]
    fn cached_capture_facts_are_rejected_for_another_program_identity() {
        let first = program(vec![proto(true, vec![Instruction::LoadArguments])]);
        let second = program(vec![proto(true, vec![Instruction::LoadArguments])]);
        let facts = ArrowArgumentsCaptureFacts::for_program(&first);
        assert!(ArgumentsAnalysis::with_arrow_capture_facts(&first, &facts).is_some());
        assert!(ArgumentsAnalysis::with_arrow_capture_facts(&second, &facts).is_none());
    }

    #[test]
    fn standalone_length_proof_rejects_proto_from_another_program() {
        let first = program(vec![proto(false, vec![Instruction::LoadArguments])]);
        let second = program(vec![proto(false, vec![Instruction::LoadArguments])]);
        let analysis = ArgumentsAnalysis::new(&first);
        let foreign_proto = &second.functions[0];
        let graph = crate::build_function_graph(foreign_proto, &second)
            .expect("the identical-index foreign proto should form a valid graph");

        let fact = analysis.argument_length_fact_for(&graph, foreign_proto);
        assert!(!fact.reads_elided);
        assert!(fact.values.is_empty());
    }
}
