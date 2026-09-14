//! Which callbacks `parallelMap` may run off the realm thread (ADR 0058, application 2).
//!
//! A chunk of `parallelMap` runs on a Rayon worker with no realm: no host, no `thrown`, no
//! thread-local heap cache, and a collector on the realm thread that assumes it is the only
//! mutator. The only closure that can run there is one that reads its `double` parameters,
//! computes, and returns a `double`. This module is the one place that decides that, from the
//! function's value graph, so the decision is a list of rules a reviewer can read and a test can
//! pin one by one, rather than a property of whatever the emitter happens to produce.

use crate::graph::{FunctionGraph, GraphOp, Terminator};
use kiln_ir::{Builtin, Constant, FunctionProto, Program, UnaryOp};

/// The `Math` members a chunk may call: each is one libm call on a double, allocates nothing,
/// and never throws. This is exactly the set `direct::intrinsic_results` proves returns a number
/// and `emit_graph::ops::call_method` emits inline as C; a name on one list and not the others
/// would either not be proven or not be inline, so keep the three together.
pub const PARALLEL_MATH: &[(&str, usize)] = &[
    ("sqrt", 1),
    ("abs", 1),
    ("floor", 1),
    ("ceil", 1),
    ("round", 1),
    ("trunc", 1),
    ("sign", 1),
    ("min", 2),
    ("max", 2),
    ("pow", 2),
];

/// Whether `graph` (the body of `proto`) may be lowered to a `parallelMap` chunk.
///
/// The rules, each pinned by a test below:
///
/// 1. The proto is a plain function or arrow: not a generator, not async, no rest parameter,
///    and declared with one or two parameters (the element, and optionally the index).
/// 2. No block has an exception handler and no terminator throws.
/// 3. Values come only from numeric constants, the parameters and locals, unary and binary
///    arithmetic and comparison, `Phi`, and `Math.<name>` calls where `<name>` with that arity
///    is in [`PARALLEL_MATH`] and the receiver is the `Math` builtin itself.
/// 4. Nothing reads or writes beyond the frame: no upvalues (even a numeric one is a heap cell
///    read), no cells, no globals, no `this`, no `new.target`, no `arguments`.
/// 5. No property access, no element access, no allocation, no call other than rule 3.
/// 6. Every reachable `return` returns a value (the plan then proves it a number; see
///    `direct::DirectPlan::plan`, which combines this with `typed_returns`).
///
/// The plan also requires that the typed entry the emitter produced for this proto needs nothing
/// but its parameters (`TypedNeeds::default()`), which is the machine's own statement that the
/// emitted C reaches no host, receiver, upvalue table or shape row. That check lives at the call
/// site, where the needs are known; this function is the source-level half.
pub fn parallel_safe(graph: &FunctionGraph, proto: &FunctionProto, program: &Program) -> bool {
    if proto.generator
        || proto.asynchronous
        || proto.async_generator
        || !(1..=2).contains(&proto.parameter_count)
        || proto
            .code
            .iter()
            .any(|i| matches!(i, kiln_ir::Instruction::RestArguments(_)))
    {
        return false;
    }
    let math: std::collections::HashSet<_> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .filter_map(|(v, op)| matches!(op, GraphOp::LoadBuiltin(Builtin::Math)).then_some(*v))
        .collect();
    for block in &graph.blocks {
        if block.exception_handler.is_some() {
            return false;
        }
        match &block.terminator {
            Terminator::Throw(_) | Terminator::Return(None) => {
                // An unreachable `return undefined` the compiler leaves after the last statement
                // is fine; one something reaches is not a number.
                let reachable = block.id == graph.entry_block || !block.predecessors.is_empty();
                if matches!(block.terminator, Terminator::Throw(_)) || reachable {
                    return false;
                }
            }
            _ => {}
        }
        for (_, op) in &block.operations {
            let ok = if op.effects() == crate::graph::Effects::PURE {
                match op {
                    GraphOp::Const { constant, .. } => matches!(
                        constant,
                        Constant::Number(_) | Constant::Boolean(_) | Constant::Undefined
                    ),
                    GraphOp::LoadLocal(_) | GraphOp::StoreLocal { .. } | GraphOp::Phi { .. } => {
                        true
                    }
                    GraphOp::LoadBuiltin(Builtin::Math) => true,
                    GraphOp::Unary { op, .. } => matches!(
                        op,
                        UnaryOp::Plus | UnaryOp::Negate | UnaryOp::Not | UnaryOp::BitwiseNot
                    ),
                    GraphOp::Binary { .. } => true,
                    _ => false,
                }
            } else if let GraphOp::CallMethod {
                object, name, args, ..
            } = op
            {
                math.contains(object)
                    && match program.constants.get(*name as usize) {
                        Some(Constant::String(key)) => PARALLEL_MATH
                            .iter()
                            .any(|(k, n)| k == key && *n == args.len()),
                        _ => false,
                    }
            } else {
                false
            };
            if !ok {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build_function_graph;
    use kiln_ir::{BYTECODE_VERSION, BinaryOp, Instruction};

    fn proto(code: Vec<Instruction>, parameter_count: u32) -> FunctionProto {
        FunctionProto {
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            declared_length: parameter_count,
            name: Some("cb".into()),
            parameter_count,
            local_count: parameter_count + 1,
            code,
            arrow: true,
            generator: false,
            asynchronous: false,
            async_generator: false,
            is_method: false,
            mapped_arguments: false,
            upvalues: Vec::new(),
            type_annotations: None,
        }
    }

    fn program(proto: &FunctionProto) -> Program {
        Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![
                Constant::Number(1.0f64.to_bits()),
                Constant::String("sqrt".into()),
                Constant::String("sin".into()),
                Constant::String("length".into()),
                Constant::String("x".into()),
            ],
            code: vec![Instruction::Return],
            local_count: 0,
            functions: vec![proto.clone()],
            template_sites: Vec::new(),
        }
    }

    fn safe(proto: FunctionProto) -> bool {
        let program = program(&proto);
        let graph = build_function_graph(&proto, &program).unwrap();
        parallel_safe(&graph, &proto, &program)
    }

    // `x => x * 2 + 1` (the constant is 1; the shape is what matters).
    fn arithmetic() -> Vec<Instruction> {
        vec![
            Instruction::LoadLocal(0),
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::Multiply),
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::Add),
            Instruction::Return,
        ]
    }

    #[test]
    fn arithmetic_over_the_parameter_qualifies() {
        assert!(safe(proto(arithmetic(), 1)));
        assert!(safe(proto(arithmetic(), 2)));
    }

    #[test]
    fn rule_1_generators_async_rest_and_arity_do_not() {
        let mut g = proto(arithmetic(), 1);
        g.generator = true;
        assert!(!safe(g));
        let mut a = proto(arithmetic(), 1);
        a.asynchronous = true;
        assert!(!safe(a));
        assert!(!safe(proto(arithmetic(), 0)));
        assert!(!safe(proto(arithmetic(), 3)));
        let mut rest = arithmetic();
        rest.insert(0, Instruction::RestArguments(0));
        rest.insert(1, Instruction::Pop);
        assert!(!safe(proto(rest, 1)));
    }

    #[test]
    fn rule_2_a_throw_does_not() {
        let code = vec![Instruction::Const(0), Instruction::Throw];
        assert!(!safe(proto(code, 1)));
    }

    #[test]
    fn rule_3_allowlisted_math_qualifies_and_other_math_does_not() {
        let sqrt = vec![
            Instruction::LoadBuiltin(Builtin::Math),
            Instruction::LoadLocal(0),
            Instruction::CallMethod { name: 1, argc: 1 },
            Instruction::Return,
        ];
        assert!(safe(proto(sqrt, 1)));
        let sin = vec![
            Instruction::LoadBuiltin(Builtin::Math),
            Instruction::LoadLocal(0),
            Instruction::CallMethod { name: 2, argc: 1 },
            Instruction::Return,
        ];
        assert!(!safe(proto(sin, 1)));
    }

    #[test]
    fn rule_4_an_upvalue_read_does_not() {
        let code = vec![
            Instruction::LoadLocal(0),
            Instruction::LoadUpvalue(0),
            Instruction::Binary(BinaryOp::Add),
            Instruction::Return,
        ];
        assert!(!safe(proto(code, 1)));
    }

    #[test]
    fn rule_4_this_does_not() {
        let code = vec![Instruction::LoadThis, Instruction::Return];
        assert!(!safe(proto(code, 1)));
    }

    #[test]
    fn rule_5_property_access_and_allocation_do_not() {
        let length = vec![
            Instruction::LoadLocal(0),
            Instruction::GetProperty(3),
            Instruction::Return,
        ];
        assert!(!safe(proto(length, 1)));
        let array = vec![
            Instruction::LoadLocal(0),
            Instruction::NewArray { count: 1 },
            Instruction::Return,
        ];
        assert!(!safe(proto(array, 1)));
    }

    #[test]
    fn rule_6_a_reachable_return_without_a_value_does_not() {
        let code = vec![Instruction::Const(0), Instruction::Pop, Instruction::Return];
        // `Return` with nothing on the stack is `return undefined` reachable from the entry.
        assert!(!safe(proto(code, 1)));
    }
}
