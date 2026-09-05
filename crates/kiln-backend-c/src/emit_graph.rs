//! C99 code generation directly from the SSA Value Graph.
//!
//! As specified in Phase 1 Step 1.3 of `fastest-native-plan.md` and ADR 0056:
//! - Eliminates the memory-bound `stack[]` array and `sp` stack pointer.
//! - Emits SSA intermediate values directly as C local variables (`kiln_val v0`, `v1`, ...).
//! - Retains `locals[]` only when needed for closure cells or generator suspension frames.
//! - Materializes call arguments into local stack arrays at call sites only, avoiding whole-body address-taking.

use crate::graph::{
    BasicBlock, BlockId, FunctionGraph, GraphOp, Terminator, ValueId, proven_numbers,
};
use kiln_ir::{BinaryOp, Constant, FunctionProto, Program, UnaryOp};
use std::collections::HashSet;
use std::fmt::Write;

/// Whether every operation in `graph` has a lowering below. This is the one place that decides
/// what the graph emitter takes; `render_function` asks it and otherwise uses the stack emitter.
pub fn can_lower(graph: &FunctionGraph) -> bool {
    graph.blocks.iter().all(|block| {
        !matches!(
            block.terminator,
            Terminator::Yield { .. } | Terminator::Await { .. }
        ) && block.operations.iter().all(|(_, op)| {
            matches!(
                op,
                GraphOp::Const { .. }
                    | GraphOp::LoadLocal(_)
                    | GraphOp::StoreLocal { .. }
                    | GraphOp::LoadCell(_)
                    | GraphOp::StoreCell { .. }
                    | GraphOp::MakeCell(_)
                    | GraphOp::LoadGlobal(_)
                    | GraphOp::StoreGlobal { .. }
                    | GraphOp::LoadThis
                    | GraphOp::LoadNewTarget
                    | GraphOp::MarkUninitialized(_)
                    | GraphOp::CheckTdz { .. }
                    | GraphOp::Unary { .. }
                    | GraphOp::Binary { .. }
                    | GraphOp::Call { .. }
                    | GraphOp::CallMethod { .. }
                    | GraphOp::NewArray { .. }
                    | GraphOp::GetProperty { .. }
                    | GraphOp::SetProperty { .. }
                    | GraphOp::SetPropertyStrict { .. }
                    | GraphOp::GetComputed { .. }
                    | GraphOp::SetComputed { .. }
                    | GraphOp::SetComputedStrict { .. }
                    | GraphOp::Phi { .. }
            )
        })
    })
}

/// Emits the C99 function body for a function proto using its SSA Value Graph.
#[allow(clippy::too_many_arguments)]
pub fn emit_function_from_graph(
    output: &mut String,
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    symbol: &str,
    index: usize,
    scalar_proto: bool,
    diagnostic_site: &str,
) {
    // 1. Function comment header
    if let Some(name) = proto.name.as_deref().filter(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$')
    }) {
        writeln!(output, "/* function {index}: {name} */").unwrap();
    }

    // 2. Signature
    writeln!(
        output,
        "static inline int {symbol}_fn_{index}(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues, const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {{"
    )
    .unwrap();

    // 3. Context & arguments setup
    if scalar_proto {
        output.push_str("    (void)ctx; (void)receiver_ptr; (void)upvalues;\n");
    } else {
        output.push_str("    kiln_host *host = ctx ? ctx->host : NULL;\n");
        output.push_str("    kiln_value *thrown = ctx ? ctx->thrown : NULL;\n");
        output.push_str("    kiln_generator *frame = ctx ? ctx->frame : NULL;\n");
        output.push_str("    int depth = ctx ? ctx->depth : 0;\n");
        output.push_str(
            "    kiln_val receiver = receiver_ptr ? *receiver_ptr : KILN_VAL_UNDEFINED;\n",
        );
        output.push_str("    (void)receiver_ptr;\n");
        writeln!(
            output,
            "    kiln_c99_frame_mapped_arguments = {};",
            i32::from(proto.mapped_arguments)
        )
        .unwrap();
    }
    output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");

    let locals = usize::try_from(proto.local_count).unwrap().max(1);
    writeln!(output, "    kiln_val locals[{locals}];").unwrap();
    writeln!(
        output,
        "    for (size_t slot = 0; slot < (size_t){locals}; ++slot) locals[slot] = KILN_VAL_UNDEFINED;"
    )
    .unwrap();

    if proto.parameter_count == 0 {
        output.push_str("    (void)args; (void)argc;\n");
    } else {
        writeln!(
            output,
            "    for (size_t slot = 0; slot < (size_t){} && slot < argc; ++slot) locals[slot] = args[slot];",
            proto.parameter_count
        )
        .unwrap();
    }

    // 4. Declare SSA value locals. A value proven to be a number also gets a `double` twin:
    // typed operations compute the double directly, with no tag test and no memcpy, and box it
    // into the `kiln_val` twin for whoever reads the value untyped. Clang deletes a box nothing
    // reads. Nothing here is a guess, so nothing here needs a guard.
    let numbers = proven_numbers(graph);
    let defs: std::collections::HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(v, op)| (*v, op)))
        .collect();
    if !numbers.is_empty() {
        let mut ids: Vec<u32> = numbers.iter().map(|v| v.0).collect();
        ids.sort_unstable();
        output.push_str("    double ");
        for (i, vid) in ids.iter().enumerate() {
            if i > 0 {
                output.push_str(", ");
            }
            write!(output, "d{vid} = 0.0").unwrap();
        }
        output.push_str(";\n");
    }
    if graph.next_value_id > 0 {
        output.push_str("    kiln_val ");
        for vid in 0..graph.next_value_id {
            if vid > 0 {
                output.push_str(", ");
            }
            write!(output, "v{vid} = KILN_VAL_UNDEFINED").unwrap();
        }
        output.push_str(";\n");
    }

    // 5. Basic blocks emission
    for block in &graph.blocks {
        writeln!(output, "kiln_block_{}:;", block.id.0).unwrap();

        for &(vid, ref op) in &block.operations {
            emit_graph_op(
                output,
                vid,
                op,
                program,
                symbol,
                diagnostic_site,
                scalar_proto,
                &numbers,
            );
        }

        emit_terminator(output, graph, block, &block.terminator, &numbers, &defs);
    }

    // 6. Unwind and refusal handlers
    output.push_str("kiln_unwind:;\n");
    output.push_str("    return KILN_C99_THROWN;\n");
    if !scalar_proto {
        output.push_str("kiln_refusal:;\n");
        output.push_str("    *result = kiln_val_from_boundary(kiln_c99_control_diagnostic());\n");
        output.push_str("    return KILN_C99_CONTROL_ERROR;\n");
    }
    output.push_str("}\n");
}

/// One status check after a runtime call. A throw unwinds; a refusal is attributed to this site
/// and reported through `kiln_refusal`, which carries the diagnostic in the result the way the
/// stack emitter does; anything else returns as it is.
fn status_check(diagnostic_site: &str, operation: &str) -> String {
    format!(
        "if (status == KILN_C99_THROWN) goto kiln_unwind; if (status == KILN_C99_CONTROL_ERROR) {{ kiln_c99_note_refusal(\"{diagnostic_site}\", (size_t)0, \"{operation}\", (size_t)kiln_c99_diagnostic_active_depth); goto kiln_refusal; }} if (status != KILN_C99_OK) return status;"
    )
}

/// The numeric fast path of a binary operator on two unboxed doubles `a` and `b`, as the C
/// expression producing a `kiln_val`, or `None` for an operator with no numeric form.
fn binary_fast_body(op: BinaryOp) -> Option<&'static str> {
    Some(match op {
        BinaryOp::Add => "kiln_c99_val_from_double(a + b)",
        BinaryOp::Subtract => "kiln_c99_val_from_double(a - b)",
        BinaryOp::Multiply => "kiln_c99_val_from_double(a * b)",
        BinaryOp::Divide => "kiln_c99_val_from_double(a / b)",
        BinaryOp::Exponentiate => "kiln_c99_val_from_double(pow(a, b))",
        // `%` on two integers in the exact range is an integer remainder; anything else is fmod,
        // which is what the language specifies for every double.
        BinaryOp::Remainder => {
            "((b > 0.0 && b <= 9007199254740992.0 && a >= 0.0 && a <= 9007199254740992.0 && a == (double)(int64_t)a && b == (double)(int64_t)b) ? kiln_c99_val_from_double((double)((int64_t)a % (int64_t)b)) : kiln_c99_val_from_double(fmod(a, b)))"
        }
        BinaryOp::StrictEqual | BinaryOp::LooseEqual => {
            "((a == b) ? KILN_VAL_TRUE : KILN_VAL_FALSE)"
        }
        BinaryOp::StrictNotEqual | BinaryOp::LooseNotEqual => {
            "((a != b) ? KILN_VAL_TRUE : KILN_VAL_FALSE)"
        }
        BinaryOp::LessThan => "((a < b) ? KILN_VAL_TRUE : KILN_VAL_FALSE)",
        BinaryOp::LessThanOrEqual => "((a <= b) ? KILN_VAL_TRUE : KILN_VAL_FALSE)",
        BinaryOp::GreaterThan => "((a > b) ? KILN_VAL_TRUE : KILN_VAL_FALSE)",
        BinaryOp::GreaterThanOrEqual => "((a >= b) ? KILN_VAL_TRUE : KILN_VAL_FALSE)",
        // The 32-bit conversions live inside the prelude and are not exported to a module
        // unit; the pure helper does them exactly and never calls back into the program when
        // both operands are numbers, which the guard around this body has just established.
        BinaryOp::BitwiseAnd => "kiln_c99_val_binary_fast(17, LEFT, RIGHT)",
        BinaryOp::BitwiseOr => "kiln_c99_val_binary_fast(18, LEFT, RIGHT)",
        BinaryOp::BitwiseXor => "kiln_c99_val_binary_fast(19, LEFT, RIGHT)",
        BinaryOp::ShiftLeft => "kiln_c99_val_binary_fast(20, LEFT, RIGHT)",
        BinaryOp::ShiftRight => "kiln_c99_val_binary_fast(21, LEFT, RIGHT)",
        BinaryOp::UnsignedShiftRight => "kiln_c99_val_binary_fast(22, LEFT, RIGHT)",
        BinaryOp::In | BinaryOp::InstanceOf => return None,
    })
}

fn binary_code(op: BinaryOp) -> u16 {
    match op {
        BinaryOp::Add => 1,
        BinaryOp::Subtract => 2,
        BinaryOp::Multiply => 3,
        BinaryOp::Divide => 4,
        BinaryOp::Remainder => 5,
        BinaryOp::Exponentiate => 6,
        BinaryOp::StrictEqual => 7,
        BinaryOp::StrictNotEqual => 8,
        BinaryOp::LessThan => 9,
        BinaryOp::LessThanOrEqual => 10,
        BinaryOp::GreaterThan => 11,
        BinaryOp::GreaterThanOrEqual => 12,
        BinaryOp::In => 13,
        BinaryOp::InstanceOf => 14,
        BinaryOp::LooseEqual => 15,
        BinaryOp::LooseNotEqual => 16,
        BinaryOp::BitwiseAnd => 17,
        BinaryOp::BitwiseOr => 18,
        BinaryOp::BitwiseXor => 19,
        BinaryOp::ShiftLeft => 20,
        BinaryOp::ShiftRight => 21,
        BinaryOp::UnsignedShiftRight => 22,
    }
}

/// The numeric fast path of a unary operator on an unboxed double `a`, or `None` when the
/// operator never takes it.
fn unary_fast_body(op: UnaryOp) -> Option<&'static str> {
    Some(match op {
        UnaryOp::Plus | UnaryOp::ToNumeric => "value",
        UnaryOp::Negate => "kiln_c99_val_from_double(-a)",
        UnaryOp::BitwiseNot => "kiln_c99_val_unary_fast(5, value)",
        UnaryOp::Increment => "kiln_c99_val_from_double(a + 1.0)",
        UnaryOp::Decrement => "kiln_c99_val_from_double(a - 1.0)",
        UnaryOp::Not | UnaryOp::TypeOf | UnaryOp::Void => return None,
    })
}

fn unary_code(op: UnaryOp) -> u16 {
    match op {
        UnaryOp::Plus => 1,
        UnaryOp::Negate => 2,
        UnaryOp::Not => 3,
        UnaryOp::TypeOf => 4,
        UnaryOp::BitwiseNot => 5,
        UnaryOp::Void => 6,
        UnaryOp::ToNumeric => 7,
        UnaryOp::Increment => 8,
        UnaryOp::Decrement => 9,
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_graph_op(
    output: &mut String,
    vid: ValueId,
    op: &GraphOp,
    program: &Program,
    symbol: &str,
    diagnostic_site: &str,
    scalar_proto: bool,
    numbers: &HashSet<ValueId>,
) {
    let v = vid.0;
    let typed = numbers.contains(&vid);
    match op {
        GraphOp::Const { index, constant } => match constant {
            Constant::Number(bits) => {
                writeln!(output, "    v{v} = UINT64_C({bits});").unwrap();
                if typed {
                    writeln!(output, "    memcpy(&d{v}, &v{v}, sizeof(d{v}));").unwrap();
                }
            }
            Constant::Undefined => {
                writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap();
            }
            Constant::Null => {
                writeln!(output, "    v{v} = KILN_VAL_NULL;").unwrap();
            }
            Constant::Boolean(b) => {
                let val_str = if *b {
                    "KILN_VAL_TRUE"
                } else {
                    "KILN_VAL_FALSE"
                };
                writeln!(output, "    v{v} = {val_str};").unwrap();
            }
            // The same lowering the stack emitter uses: a NUL-free literal is a tagged pointer to
            // its static bytes; one with a NUL inside goes through the per-unit string cache; a
            // bigint is parsed once into a cached heap value.
            Constant::String(value) => {
                if value.contains('\0') {
                    writeln!(
                        output,
                        "    v{v} = kiln_c99_string_constant(&{symbol}_string_{index}, {symbol}_constant_{index}, {});",
                        value.len()
                    )
                    .unwrap();
                } else {
                    writeln!(
                        output,
                        "    v{v} = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, {symbol}_constant_{index});"
                    )
                    .unwrap();
                }
            }
            Constant::BigInt(digits) => {
                writeln!(
                    output,
                    "    {{ kiln_value tmp; int status = kiln_c99_bigint_constant(&{symbol}_bigint_{index}, {symbol}_constant_{index}, {}, &tmp); if (status != KILN_C99_OK) return status; v{v} = kiln_val_from_boundary(tmp); }}",
                    digits.len()
                )
                .unwrap();
            }
        },
        GraphOp::LoadLocal(slot) => {
            writeln!(output, "    v{v} = locals[{slot}];").unwrap();
        }
        GraphOp::StoreLocal { slot, value } => {
            writeln!(output, "    locals[{slot}] = v{};", value.0).unwrap();
            writeln!(output, "    v{v} = v{};", value.0).unwrap();
            if typed {
                writeln!(output, "    d{v} = d{};", value.0).unwrap();
            }
        }
        // A cell is a shared box, the same one the stack emitter makes and reads through the
        // same helpers: the load checks the temporal dead zone, the store writes the box.
        GraphOp::LoadCell(slot) => {
            let check = status_check(diagnostic_site, "LoadCell");
            writeln!(
                output,
                "    {{ kiln_val cell_stack[1]; size_t cell_sp = 0; int status = kiln_c99_op_val_load_cell(locals[{slot}], cell_stack, &cell_sp, thrown); {check} v{v} = cell_stack[0]; }}"
            )
            .unwrap();
        }
        GraphOp::StoreCell { slot, value } => {
            writeln!(
                output,
                "    *(kiln_val *)KILN_VAL_PTR(locals[{slot}]) = v{};",
                value.0
            )
            .unwrap();
            writeln!(output, "    v{v} = v{};", value.0).unwrap();
            if typed {
                writeln!(output, "    d{v} = d{};", value.0).unwrap();
            }
        }
        GraphOp::MakeCell(slot) => {
            writeln!(
                output,
                "    {{ int status = kiln_c99_val_make_cell(&locals[{slot}]); if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
            writeln!(output, "    v{v} = locals[{slot}];").unwrap();
        }
        GraphOp::LoadGlobal(slot) => {
            writeln!(output, "    v{v} = {symbol}_globals[{slot}];").unwrap();
        }
        GraphOp::StoreGlobal { slot, value } => {
            writeln!(output, "    {symbol}_globals[{slot}] = v{};", value.0).unwrap();
            writeln!(output, "    v{v} = v{};", value.0).unwrap();
            if typed {
                writeln!(output, "    d{v} = d{};", value.0).unwrap();
            }
        }
        GraphOp::LoadThis => {
            writeln!(output, "    v{v} = receiver;").unwrap();
        }
        GraphOp::LoadNewTarget => {
            writeln!(
                output,
                "    v{v} = kiln_val_from_boundary(kiln_c99_new_target);"
            )
            .unwrap();
        }
        GraphOp::MarkUninitialized(slot) => {
            writeln!(output, "    locals[{slot}] = KILN_VAL_UNINITIALIZED;").unwrap();
            writeln!(output, "    v{v} = KILN_VAL_UNINITIALIZED;").unwrap();
        }
        GraphOp::CheckTdz { slot: _, value } => {
            writeln!(
                output,
                "    {{ int status = kiln_c99_val_check_tdz(v{}, thrown); if (status == KILN_C99_THROWN) goto kiln_unwind; if (status != KILN_C99_OK) return status; }}",
                value.0
            )
            .unwrap();
            writeln!(output, "    v{v} = v{};", value.0).unwrap();
            if typed {
                writeln!(output, "    d{v} = d{};", value.0).unwrap();
            }
        }
        GraphOp::Unary { op, value } if typed => {
            // Proven number in, proven number out: the double is computed directly.
            let x = value.0;
            let fast = unary_fast_body(*op)
                .expect("a proven-number unary has a numeric body")
                .replace("value", &format!("v{x}"))
                .replace("kiln_c99_val_from_double(", "(")
                .replace(
                    "kiln_c99_val_unary_fast(5, v",
                    "kiln_c99_val_unary_fast(5, v",
                );
            match op {
                UnaryOp::Plus | UnaryOp::ToNumeric => {
                    writeln!(output, "    d{v} = d{x}; v{v} = v{x};").unwrap();
                }
                UnaryOp::BitwiseNot => {
                    writeln!(output, "    v{v} = kiln_c99_val_unary_fast(5, v{x}); memcpy(&d{v}, &v{v}, sizeof(d{v}));").unwrap();
                }
                _ => {
                    let expr = fast.replace('a', &format!("d{x}"));
                    writeln!(
                        output,
                        "    d{v} = {expr}; v{v} = kiln_c99_val_from_double(d{v});"
                    )
                    .unwrap();
                }
            }
        }
        GraphOp::Unary { op, value } => {
            let x = value.0;
            let code = unary_code(*op);
            match op {
                UnaryOp::Void => writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap(),
                // `!` and `typeof` never call back into the program and never throw.
                UnaryOp::Not => writeln!(
                    output,
                    "    v{v} = kiln_c99_val_truthy(v{x}) ? KILN_VAL_FALSE : KILN_VAL_TRUE;"
                )
                .unwrap(),
                UnaryOp::TypeOf => {
                    writeln!(output, "    v{v} = kiln_c99_val_unary_fast({code}, v{x});").unwrap()
                }
                _ => {
                    let fast = unary_fast_body(*op)
                        .unwrap()
                        .replace("value", &format!("v{x}"));
                    // The slow path converts on the machine: an object operand's `valueOf` runs
                    // and may throw. A body that cannot produce a reference never gets there,
                    // and takes the pure helper the way the stack emitter does.
                    let slow = if scalar_proto {
                        format!("v{v} = kiln_c99_val_unary_fast({code}, v{x});")
                    } else {
                        let check = status_check(diagnostic_site, "Unary");
                        format!(
                            "kiln_value ans; int status = kiln_c99_unary_checked({code}, host, kiln_val_to_boundary(v{x}), &ans, depth, thrown); {check} v{v} = kiln_val_from_boundary(ans);"
                        )
                    };
                    writeln!(
                        output,
                        "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{x}))) {{ double a; memcpy(&a, &v{x}, sizeof(a)); (void)a; v{v} = {fast}; }} else {{ {slow} }}"
                    )
                    .unwrap();
                }
            }
        }
        GraphOp::Binary { op, left, right }
            if numbers.contains(left) && numbers.contains(right) =>
        {
            // Both operands proven numbers: no tag test, no memcpy. A comparison yields the
            // boolean directly; arithmetic computes the double twin and boxes it.
            let (l, r) = (left.0, right.0);
            let (a, b) = (format!("d{l}"), format!("d{r}"));
            match op {
                BinaryOp::StrictEqual | BinaryOp::LooseEqual => writeln!(output, "    v{v} = ({a} == {b}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;").unwrap(),
                BinaryOp::StrictNotEqual | BinaryOp::LooseNotEqual => writeln!(output, "    v{v} = ({a} != {b}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;").unwrap(),
                BinaryOp::LessThan => writeln!(output, "    v{v} = ({a} < {b}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;").unwrap(),
                BinaryOp::LessThanOrEqual => writeln!(output, "    v{v} = ({a} <= {b}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;").unwrap(),
                BinaryOp::GreaterThan => writeln!(output, "    v{v} = ({a} > {b}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;").unwrap(),
                BinaryOp::GreaterThanOrEqual => writeln!(output, "    v{v} = ({a} >= {b}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;").unwrap(),
                BinaryOp::Add => writeln!(output, "    d{v} = {a} + {b}; v{v} = kiln_c99_val_from_double(d{v});").unwrap(),
                BinaryOp::Subtract => writeln!(output, "    d{v} = {a} - {b}; v{v} = kiln_c99_val_from_double(d{v});").unwrap(),
                BinaryOp::Multiply => writeln!(output, "    d{v} = {a} * {b}; v{v} = kiln_c99_val_from_double(d{v});").unwrap(),
                BinaryOp::Divide => writeln!(output, "    d{v} = {a} / {b}; v{v} = kiln_c99_val_from_double(d{v});").unwrap(),
                BinaryOp::Exponentiate => writeln!(output, "    d{v} = pow({a}, {b}); v{v} = kiln_c99_val_from_double(d{v});").unwrap(),
                BinaryOp::Remainder => writeln!(output, "    d{v} = ({b} > 0.0 && {b} <= 9007199254740992.0 && {a} >= 0.0 && {a} <= 9007199254740992.0 && {a} == (double)(int64_t){a} && {b} == (double)(int64_t){b}) ? (double)((int64_t){a} % (int64_t){b}) : fmod({a}, {b}); v{v} = kiln_c99_val_from_double(d{v});").unwrap(),
                BinaryOp::BitwiseAnd | BinaryOp::BitwiseOr | BinaryOp::BitwiseXor | BinaryOp::ShiftLeft | BinaryOp::ShiftRight | BinaryOp::UnsignedShiftRight => {
                    writeln!(output, "    v{v} = kiln_c99_val_binary_fast({}, v{l}, v{r}); memcpy(&d{v}, &v{v}, sizeof(d{v}));", binary_code(*op)).unwrap()
                }
                BinaryOp::In | BinaryOp::InstanceOf => unreachable!("`in` and `instanceof` are never proven numeric"),
            }
        }
        GraphOp::Binary { op, left, right } => {
            let (l, r) = (left.0, right.0);
            let code = binary_code(*op);
            // The slow path runs the program's own conversions and can throw; the pure helper
            // is only for a body that cannot hold a reference (the stack emitter's `callables`).
            let slow = if scalar_proto {
                format!("v{v} = kiln_c99_val_binary_fast({code}, v{l}, v{r});")
            } else {
                let check = status_check(diagnostic_site, "Binary");
                format!(
                    "int status = KILN_C99_OK; v{v} = kiln_c99_val_binary({code}, host, v{l}, v{r}, &status, depth, thrown); {check}"
                )
            };
            let fast = binary_fast_body(*op).map(|f| {
                f.replace("LEFT", &format!("v{l}"))
                    .replace("RIGHT", &format!("v{r}"))
            });
            match fast {
                Some(fast) => writeln!(
                    output,
                    "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); v{v} = {fast}; }} else {{ {slow} }}"
                )
                .unwrap(),
                None => writeln!(output, "    {{ {slow} }}").unwrap(),
            }
        }
        // A call is the stack emitter's direct path on `kiln_val`: a plain closure that
        // publishes no frame is entered directly, a body this unit compiled is called by name so
        // the C compiler may inline it, and everything else goes through the runtime.
        GraphOp::Call { callee, args, .. } => {
            let argc = args.len();
            let mut init = format!("v{}", callee.0);
            for arg in args {
                init.push_str(&format!(", v{}", arg.0));
            }
            let mut static_branches = String::new();
            if program.functions.len() <= 32 {
                for i in 0..program.functions.len() {
                    let fn_name = format!("{symbol}_fn_{i}");
                    let keyword = if static_branches.is_empty() {
                        "if"
                    } else {
                        " else if"
                    };
                    static_branches.push_str(&format!(
                        "{keyword} (plain->body == (kiln_body_fn){fn_name}) {{ status = {fn_name}(&ctx_call, &call_result, plain->upvalues, &recv, call_args, (size_t){argc}); }}"
                    ));
                }
            }
            let indirect = if static_branches.is_empty() {
                "status = plain->body(&ctx_call, &call_result, plain->upvalues, &recv, call_args, (size_t)ARGC);".replace("ARGC", &argc.to_string())
            } else {
                format!(
                    "{static_branches} else {{ status = plain->body(&ctx_call, &call_result, plain->upvalues, &recv, call_args, (size_t){argc}); }}"
                )
            };
            let check = status_check(diagnostic_site, "Call");
            writeln!(
                output,
                "    {{ kiln_val call_stack[{n}] = {{ {init} }}; size_t call_sp = {n}; kiln_closure *plain = KILN_VAL_IS_CLOSURE(call_stack[0]) ? (kiln_closure *)KILN_VAL_PTR(call_stack[0]) : NULL; if (KILN_LIKELY(plain != NULL && !plain->publishes_frame && !plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH)) {{ kiln_val call_result = KILN_VAL_UNDEFINED; const kiln_val *call_args = {argc} > 0 ? &call_stack[1] : NULL; kiln_val recv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED; if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&recv); kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; int status; {indirect} if (status == KILN_C99_THROWN) goto kiln_unwind; if (status != KILN_C99_OK) return status; v{v} = call_result; }} else {{ int status = kiln_c99_op_val_call(host, call_stack, &call_sp, (size_t){argc}, depth, thrown, \"{diagnostic_site}\", (size_t)0, \"Call\"); {check} v{v} = call_stack[0]; }} }}",
                n = argc + 1
            )
            .unwrap();
        }
        GraphOp::CallMethod {
            object, name, args, ..
        } => {
            let argc = args.len();
            let key_len = match program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.len(),
                _ => 0,
            };
            let mut init = format!("v{}", object.0);
            for arg in args {
                init.push_str(&format!(", v{}", arg.0));
            }
            let check = status_check(diagnostic_site, "CallMethod");
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; kiln_val call_stack[{n}] = {{ {init} }}; size_t call_sp = {n}; int status = kiln_c99_op_val_call_method(host, call_stack, &call_sp, {symbol}_key_{name}, {key_len}, (size_t){argc}, depth, thrown, &site, \"{diagnostic_site}\", (size_t)0, \"CallMethod\"); {check} v{v} = call_stack[0]; }}",
                n = argc + 1
            )
            .unwrap();
        }
        GraphOp::NewArray { elements, .. } => {
            let mut pushes = String::new();
            for element in elements {
                pushes.push_str(&format!(
                    " status = kiln_c99_array_push(built, kiln_val_to_boundary(v{})); if (status != KILN_C99_OK) return status;",
                    element.0
                ));
            }
            writeln!(
                output,
                "    {{ kiln_value built; int status = kiln_c99_new_array(&built); if (status != KILN_C99_OK) return status;{pushes} v{v} = kiln_val_from_boundary(built); }}"
            )
            .unwrap();
        }
        GraphOp::GetProperty { object, name, .. } => {
            let key_len = match program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.len(),
                _ => 0,
            };
            let check = status_check(diagnostic_site, "GetProperty");
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; kiln_val site_stack[1] = {{ v{} }}; size_t site_sp = 1; int status = kiln_c99_op_val_get_property(host, site_stack, &site_sp, {symbol}_key_{name}, {key_len}, depth, thrown, &site); {check} v{v} = site_stack[0]; }}",
                object.0
            )
            .unwrap();
        }
        GraphOp::SetProperty {
            object,
            name,
            value,
            ..
        }
        | GraphOp::SetPropertyStrict {
            object,
            name,
            value,
            ..
        } => {
            let key_len = match program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.len(),
                _ => 0,
            };
            let strict = matches!(op, GraphOp::SetPropertyStrict { strict: true, .. }) as u8;
            let check = status_check(diagnostic_site, "SetProperty");
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; kiln_val site_stack[2] = {{ v{}, v{} }}; size_t site_sp = 2; int status = kiln_c99_op_val_set_property(host, site_stack, &site_sp, {symbol}_key_{name}, {key_len}, {strict}, depth, thrown, &site); {check} v{v} = v{}; }}",
                object.0, value.0, value.0
            )
            .unwrap();
        }
        GraphOp::GetComputed { object, key, .. } => {
            let check = status_check(diagnostic_site, "GetComputed");
            writeln!(
                output,
                "    {{ kiln_val site_stack[2] = {{ v{}, v{} }}; size_t site_sp = 2; int status = kiln_c99_op_val_get_computed(host, site_stack, &site_sp, depth, thrown); {check} v{v} = site_stack[0]; }}",
                object.0, key.0
            )
            .unwrap();
        }
        GraphOp::SetComputed {
            object, key, value, ..
        }
        | GraphOp::SetComputedStrict {
            object, key, value, ..
        } => {
            let strict = matches!(op, GraphOp::SetComputedStrict { strict: true, .. }) as u8;
            let check = status_check(diagnostic_site, "SetComputed");
            writeln!(
                output,
                "    {{ kiln_val site_stack[3] = {{ v{}, v{}, v{} }}; size_t site_sp = 3; int status = kiln_c99_op_val_set_computed(host, site_stack, &site_sp, {strict}, depth, thrown); {check} v{v} = v{}; }}",
                object.0, key.0, value.0, value.0
            )
            .unwrap();
        }
        GraphOp::Phi { .. } => {
            // Phis are resolved by incoming jumps/branches in predecessor blocks.
        }
        other => {
            // Every operation `can_emit_from_graph` admits has an arm above. Reaching here is a
            // compiler bug, and the one answer it must never produce is a silent `undefined`:
            // that is the failure this backend refuses by construction (`KILN_C99_ABSENT`).
            unreachable!("graph emitter has no lowering for {other:?}");
        }
    }
}

fn emit_terminator(
    output: &mut String,
    graph: &FunctionGraph,
    current_block: &BasicBlock,
    terminator: &Terminator,
    numbers: &HashSet<ValueId>,
    defs: &std::collections::HashMap<ValueId, &GraphOp>,
) {
    match terminator {
        Terminator::Return(Some(val)) => {
            writeln!(output, "    *result = v{}; return KILN_C99_OK;", val.0).unwrap();
        }
        Terminator::Return(None) => {
            output.push_str("    *result = KILN_VAL_UNDEFINED; return KILN_C99_OK;\n");
        }
        Terminator::Throw(val) => {
            writeln!(
                output,
                "    *thrown = kiln_val_to_boundary(v{}); goto kiln_unwind;",
                val.0
            )
            .unwrap();
        }
        Terminator::Jump(target) => {
            emit_phi_assignments(output, graph, current_block.id, *target, numbers);
            writeln!(output, "    goto kiln_block_{};", target.0).unwrap();
        }
        Terminator::Branch {
            condition,
            true_block,
            false_block,
        } => {
            // A comparison of two proven numbers branches on the doubles directly; anything
            // else asks the value for its truthiness.
            let test = match defs.get(condition) {
                Some(GraphOp::Binary { op, left, right })
                    if numbers.contains(left) && numbers.contains(right) =>
                {
                    let sym = match op {
                        BinaryOp::LessThan => Some("<"),
                        BinaryOp::LessThanOrEqual => Some("<="),
                        BinaryOp::GreaterThan => Some(">"),
                        BinaryOp::GreaterThanOrEqual => Some(">="),
                        BinaryOp::StrictEqual | BinaryOp::LooseEqual => Some("=="),
                        BinaryOp::StrictNotEqual | BinaryOp::LooseNotEqual => Some("!="),
                        _ => None,
                    };
                    sym.map(|sym| format!("d{} {sym} d{}", left.0, right.0))
                }
                _ => None,
            }
            .unwrap_or_else(|| format!("kiln_c99_val_truthy(v{})", condition.0));
            writeln!(output, "    if ({test}) {{").unwrap();
            emit_phi_assignments(output, graph, current_block.id, *true_block, numbers);
            writeln!(output, "        goto kiln_block_{};", true_block.0).unwrap();
            output.push_str("    } else {\n");
            emit_phi_assignments(output, graph, current_block.id, *false_block, numbers);
            writeln!(output, "        goto kiln_block_{};", false_block.0).unwrap();
            output.push_str("    }\n");
        }
        Terminator::Yield { resume, .. } | Terminator::Await { resume, .. } => {
            writeln!(output, "    goto kiln_block_{};", resume.0).unwrap();
        }
        Terminator::Unreachable => {
            output.push_str("    goto kiln_refusal;\n");
        }
    }
}

fn emit_phi_assignments(
    output: &mut String,
    graph: &FunctionGraph,
    from: BlockId,
    target: BlockId,
    numbers: &HashSet<ValueId>,
) {
    // A parallel move: every input is read before any phi is written, so two phis that swap
    // values (`[a, b] = [b, a]` across a back edge) do not clobber each other.
    let Some(target_block) = graph.block(target) else {
        return;
    };
    let mut moves: Vec<(u32, u32)> = Vec::new();
    for &(phi_vid, ref op) in &target_block.operations {
        if let GraphOp::Phi { inputs } = op {
            for &(pred, in_vid) in inputs {
                if pred == from && in_vid != phi_vid {
                    moves.push((phi_vid.0, in_vid.0));
                }
            }
        }
    }
    if moves.is_empty() {
        return;
    }
    output.push_str("        {");
    for (i, &(_, from_v)) in moves.iter().enumerate() {
        write!(output, " kiln_val phi_t{i} = v{from_v};").unwrap();
    }
    for (i, &(phi_v, from_v)) in moves.iter().enumerate() {
        write!(output, " v{phi_v} = phi_t{i};").unwrap();
        if numbers.contains(&ValueId(phi_v)) {
            // A proven-number phi carries its double twin across the edge too.
            write!(output, " d{phi_v} = d{from_v};").unwrap();
        }
    }
    output.push_str(" }\n");
}
