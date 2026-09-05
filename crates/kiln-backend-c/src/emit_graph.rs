//! C99 code generation directly from the SSA Value Graph.
//!
//! As specified in Phase 1 Step 1.3 of `fastest-native-plan.md` and ADR 0056:
//! - Eliminates the memory-bound `stack[]` array and `sp` stack pointer.
//! - Emits SSA intermediate values directly as C local variables (`kiln_val v0`, `v1`, ...).
//! - Retains `locals[]` only when needed for closure cells or generator suspension frames.
//! - Materializes call arguments into local stack arrays at call sites only, avoiding whole-body address-taking.

use crate::graph::{BasicBlock, BlockId, FunctionGraph, GraphOp, Terminator, ValueId};
use kiln_ir::{BinaryOp, Constant, FunctionProto, Program, UnaryOp};
use std::fmt::Write;

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

    // 4. Declare SSA value locals
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

        for &(vid, ref op, _gtype) in &block.operations {
            emit_graph_op(
                output,
                vid,
                op,
                program,
                symbol,
                diagnostic_site,
                scalar_proto,
            );
        }

        emit_terminator(output, graph, block, &block.terminator);
    }

    // 6. Unwind and refusal handlers
    output.push_str("kiln_unwind:;\n");
    output.push_str("    return KILN_C99_THROWN;\n");
    if !scalar_proto {
        output.push_str("kiln_refusal:;\n");
        output.push_str("    return KILN_C99_CONTROL_ERROR;\n");
    }
    output.push_str("}\n");
}

fn emit_graph_op(
    output: &mut String,
    vid: ValueId,
    op: &GraphOp,
    program: &Program,
    symbol: &str,
    diagnostic_site: &str,
    _scalar_proto: bool,
) {
    let v = vid.0;
    match op {
        GraphOp::Const { index, constant } => match constant {
            Constant::Number(bits) => {
                writeln!(output, "    v{v} = UINT64_C({bits});").unwrap();
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
            Constant::String(_) => {
                writeln!(output, "    v{v} = {symbol}_constant_{index};").unwrap();
            }
            Constant::BigInt(_) => {
                writeln!(output, "    v{v} = {symbol}_constant_{index};").unwrap();
            }
        },
        GraphOp::LoadLocal(slot) => {
            writeln!(output, "    v{v} = locals[{slot}];").unwrap();
        }
        GraphOp::StoreLocal { slot, value } => {
            writeln!(output, "    locals[{slot}] = v{};", value.0).unwrap();
            writeln!(output, "    v{v} = v{};", value.0).unwrap();
        }
        GraphOp::LoadCell(slot) => {
            writeln!(
                output,
                "    v{v} = kiln_c99_op_load_cell(host, locals[{slot}]);"
            )
            .unwrap();
        }
        GraphOp::StoreCell { slot, value } => {
            writeln!(
                output,
                "    kiln_c99_op_store_cell(host, locals[{slot}], v{});",
                value.0
            )
            .unwrap();
            writeln!(output, "    v{v} = v{};", value.0).unwrap();
        }
        GraphOp::MakeCell(slot) => {
            writeln!(
                output,
                "    locals[{slot}] = kiln_c99_op_make_cell(host, locals[{slot}]);"
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
        GraphOp::LoadArguments => {
            writeln!(
                output,
                "    v{v} = kiln_c99_op_load_arguments(host, ctx, args, argc);"
            )
            .unwrap();
        }
        GraphOp::RestArguments(offset) => {
            writeln!(
                output,
                "    v{v} = kiln_c99_op_rest_arguments(host, args, argc, {offset});"
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
        }
        GraphOp::Unary { op, value } => match op {
            UnaryOp::Not => {
                writeln!(
                    output,
                    "    v{v} = kiln_c99_val_from_bool(!kiln_c99_val_truthy(v{}));",
                    value.0
                )
                .unwrap();
            }
            UnaryOp::Plus => {
                writeln!(
                    output,
                    "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{}))) {{ v{v} = v{}; }} else {{ v{v} = kiln_c99_val_unary_fast(1, v{}); }}",
                    value.0, value.0, value.0
                )
                .unwrap();
            }
            UnaryOp::Negate => {
                writeln!(
                    output,
                    "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{}))) {{ double a; memcpy(&a, &v{}, sizeof(a)); v{v} = kiln_c99_val_from_double(-a); }} else {{ v{v} = kiln_c99_val_unary_fast(2, v{}); }}",
                    value.0, value.0, value.0
                )
                .unwrap();
            }
            UnaryOp::BitwiseNot => {
                writeln!(
                    output,
                    "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{}))) {{ double a; memcpy(&a, &v{}, sizeof(a)); v{v} = kiln_c99_val_from_double((double)(~(int32_t)a)); }} else {{ v{v} = kiln_c99_val_unary_fast(5, v{}); }}",
                    value.0, value.0, value.0
                )
                .unwrap();
            }
            UnaryOp::Void => {
                writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap();
            }
            UnaryOp::Increment => {
                writeln!(
                    output,
                    "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{}))) {{ double a; memcpy(&a, &v{}, sizeof(a)); v{v} = kiln_c99_val_from_double(a + 1.0); }} else {{ v{v} = kiln_c99_val_unary_fast(8, v{}); }}",
                    value.0, value.0, value.0
                )
                .unwrap();
            }
            UnaryOp::Decrement => {
                writeln!(
                    output,
                    "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{}))) {{ double a; memcpy(&a, &v{}, sizeof(a)); v{v} = kiln_c99_val_from_double(a - 1.0); }} else {{ v{v} = kiln_c99_val_unary_fast(9, v{}); }}",
                    value.0, value.0, value.0
                )
                .unwrap();
            }
            UnaryOp::ToNumeric => {
                writeln!(
                    output,
                    "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{}))) {{ v{v} = v{}; }} else {{ v{v} = kiln_c99_val_unary_fast(7, v{}); }}",
                    value.0, value.0, value.0
                )
                .unwrap();
            }
            UnaryOp::TypeOf => {
                writeln!(output, "    v{v} = kiln_c99_val_typeof(v{});", value.0).unwrap();
            }
        },
        GraphOp::Binary { op, left, right } => {
            let l = left.0;
            let r = right.0;
            let op_code = match op {
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
            };

            match op {
                BinaryOp::Add => {
                    writeln!(
                        output,
                        "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); v{v} = kiln_c99_val_from_double(a + b); }} else {{ v{v} = kiln_c99_val_binary_fast({op_code}, v{l}, v{r}); }}"
                    )
                    .unwrap();
                }
                BinaryOp::Subtract => {
                    writeln!(
                        output,
                        "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); v{v} = kiln_c99_val_from_double(a - b); }} else {{ v{v} = kiln_c99_val_binary_fast({op_code}, v{l}, v{r}); }}"
                    )
                    .unwrap();
                }
                BinaryOp::Multiply => {
                    writeln!(
                        output,
                        "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); v{v} = kiln_c99_val_from_double(a * b); }} else {{ v{v} = kiln_c99_val_binary_fast({op_code}, v{l}, v{r}); }}"
                    )
                    .unwrap();
                }
                BinaryOp::Divide => {
                    writeln!(
                        output,
                        "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); v{v} = kiln_c99_val_from_double(a / b); }} else {{ v{v} = kiln_c99_val_binary_fast({op_code}, v{l}, v{r}); }}"
                    )
                    .unwrap();
                }
                BinaryOp::Remainder => {
                    writeln!(
                        output,
                        "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); if (KILN_LIKELY(b > 0.0 && b <= 9007199254740992.0 && a >= 0.0 && a <= 9007199254740992.0 && a == (double)(int64_t)a && b == (double)(int64_t)b && (int64_t)b > 0)) {{ v{v} = kiln_c99_val_from_double((double)((int64_t)a % (int64_t)b)); }} else {{ v{v} = kiln_c99_val_from_double(fmod(a, b)); }} }} else {{ v{v} = kiln_c99_val_binary_fast({op_code}, v{l}, v{r}); }}"
                    )
                    .unwrap();
                }
                BinaryOp::StrictEqual => {
                    writeln!(
                        output,
                        "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); v{v} = (a == b) ? KILN_VAL_TRUE : KILN_VAL_FALSE; }} else {{ v{v} = kiln_c99_val_binary_fast({op_code}, v{l}, v{r}); }}"
                    )
                    .unwrap();
                }
                BinaryOp::StrictNotEqual => {
                    writeln!(
                        output,
                        "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); v{v} = (a != b) ? KILN_VAL_TRUE : KILN_VAL_FALSE; }} else {{ v{v} = kiln_c99_val_binary_fast({op_code}, v{l}, v{r}); }}"
                    )
                    .unwrap();
                }
                BinaryOp::LessThan => {
                    writeln!(
                        output,
                        "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); v{v} = (a < b) ? KILN_VAL_TRUE : KILN_VAL_FALSE; }} else {{ v{v} = kiln_c99_val_binary_fast({op_code}, v{l}, v{r}); }}"
                    )
                    .unwrap();
                }
                BinaryOp::LessThanOrEqual => {
                    writeln!(
                        output,
                        "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); v{v} = (a <= b) ? KILN_VAL_TRUE : KILN_VAL_FALSE; }} else {{ v{v} = kiln_c99_val_binary_fast({op_code}, v{l}, v{r}); }}"
                    )
                    .unwrap();
                }
                BinaryOp::GreaterThan => {
                    writeln!(
                        output,
                        "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); v{v} = (a > b) ? KILN_VAL_TRUE : KILN_VAL_FALSE; }} else {{ v{v} = kiln_c99_val_binary_fast({op_code}, v{l}, v{r}); }}"
                    )
                    .unwrap();
                }
                BinaryOp::GreaterThanOrEqual => {
                    writeln!(
                        output,
                        "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); v{v} = (a >= b) ? KILN_VAL_TRUE : KILN_VAL_FALSE; }} else {{ v{v} = kiln_c99_val_binary_fast({op_code}, v{l}, v{r}); }}"
                    )
                    .unwrap();
                }
                _ => {
                    writeln!(
                        output,
                        "    v{v} = kiln_c99_val_binary_fast({op_code}, v{l}, v{r});"
                    )
                    .unwrap();
                }
            }
        }
        GraphOp::Call { callee, args, .. } => {
            let argc = args.len();
            writeln!(output, "    {{").unwrap();
            writeln!(
                output,
                "        kiln_value call_stack[{}];",
                (argc + 1).max(1)
            )
            .unwrap();
            writeln!(output, "        size_t call_sp = {};", argc + 1).unwrap();
            writeln!(
                output,
                "        call_stack[0] = kiln_val_to_boundary(v{});",
                callee.0
            )
            .unwrap();
            for (i, arg) in args.iter().enumerate() {
                writeln!(
                    output,
                    "        call_stack[{}] = kiln_val_to_boundary(v{});",
                    i + 1,
                    arg.0
                )
                .unwrap();
            }
            writeln!(
                output,
                "        int status = kiln_c99_op_call(host, call_stack, &call_sp, {argc}, depth, thrown, \"{diagnostic_site}\", 0, \"Call\", NULL);"
            )
            .unwrap();
            writeln!(
                output,
                "        if (status == KILN_C99_THROWN) goto kiln_unwind;"
            )
            .unwrap();
            writeln!(output, "        if (status != KILN_C99_OK) return status;").unwrap();
            writeln!(
                output,
                "        v{v} = kiln_val_from_boundary(call_stack[0]);"
            )
            .unwrap();
            writeln!(output, "    }}").unwrap();
        }
        GraphOp::CallMethod {
            object, name, args, ..
        } => {
            let argc = args.len();
            let key_str = match program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.as_str(),
                _ => "",
            };
            writeln!(output, "    {{").unwrap();
            writeln!(
                output,
                "        kiln_value call_stack[{}];",
                (argc + 1).max(1)
            )
            .unwrap();
            writeln!(output, "        size_t call_sp = {};", argc + 1).unwrap();
            writeln!(
                output,
                "        call_stack[0] = kiln_val_to_boundary(v{});",
                object.0
            )
            .unwrap();
            for (i, arg) in args.iter().enumerate() {
                writeln!(
                    output,
                    "        call_stack[{}] = kiln_val_to_boundary(v{});",
                    i + 1,
                    arg.0
                )
                .unwrap();
            }
            writeln!(
                output,
                "        int status = kiln_c99_op_call_method(host, call_stack, &call_sp, \"{key_str}\", {}, {argc}, depth, thrown, NULL, \"{diagnostic_site}\", 0, \"CallMethod\", NULL);",
                key_str.len()
            )
            .unwrap();
            writeln!(
                output,
                "        if (status == KILN_C99_THROWN) goto kiln_unwind;"
            )
            .unwrap();
            writeln!(output, "        if (status != KILN_C99_OK) return status;").unwrap();
            writeln!(
                output,
                "        v{v} = kiln_val_from_boundary(call_stack[0]);"
            )
            .unwrap();
            writeln!(output, "    }}").unwrap();
        }
        GraphOp::MakeFunction { index, .. } => {
            writeln!(
                output,
                "    v{v} = kiln_c99_op_make_function_val(host, ctx, {index}, upvalues);"
            )
            .unwrap();
        }
        GraphOp::NewArray { elements, .. } => {
            let count = elements.len();
            if count == 0 {
                writeln!(
                    output,
                    "    v{v} = kiln_c99_op_new_array_val(host, NULL, 0);"
                )
                .unwrap();
            } else {
                writeln!(output, "    {{").unwrap();
                writeln!(output, "        kiln_val elems[{count}];").unwrap();
                for (i, elem) in elements.iter().enumerate() {
                    writeln!(output, "        elems[{i}] = v{};", elem.0).unwrap();
                }
                writeln!(
                    output,
                    "        v{v} = kiln_c99_op_new_array_val(host, elems, {count});"
                )
                .unwrap();
                writeln!(output, "    }}").unwrap();
            }
        }
        GraphOp::GetProperty { object, name, .. } => {
            let key_str = match program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.as_str(),
                _ => "",
            };
            writeln!(output, "    {{").unwrap();
            writeln!(
                output,
                "        kiln_val site_stack[1] = {{ v{} }};",
                object.0
            )
            .unwrap();
            writeln!(output, "        size_t site_sp = 1;").unwrap();
            writeln!(
                output,
                "        static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}};"
            )
            .unwrap();
            writeln!(
                output,
                "        int status = kiln_c99_op_val_get_property(host, site_stack, &site_sp, {symbol}_key_{name}, {}, depth, thrown, &site);",
                key_str.len()
            )
            .unwrap();
            writeln!(
                output,
                "        if (status == KILN_C99_THROWN) goto kiln_unwind;"
            )
            .unwrap();
            writeln!(output, "        if (status != KILN_C99_OK) return status;").unwrap();
            writeln!(output, "        v{v} = site_stack[0];").unwrap();
            writeln!(output, "    }}").unwrap();
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
            let key_str = match program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.as_str(),
                _ => "",
            };
            let strict_val = if let GraphOp::SetPropertyStrict { strict, .. } = op {
                if *strict { 1 } else { 0 }
            } else {
                0
            };
            writeln!(output, "    {{").unwrap();
            writeln!(
                output,
                "        kiln_val site_stack[2] = {{ v{}, v{} }};",
                object.0, value.0
            )
            .unwrap();
            writeln!(output, "        size_t site_sp = 2;").unwrap();
            writeln!(
                output,
                "        static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}};"
            )
            .unwrap();
            writeln!(
                output,
                "        int status = kiln_c99_op_val_set_property(host, site_stack, &site_sp, {symbol}_key_{name}, {}, {strict_val}, depth, thrown, &site);",
                key_str.len()
            )
            .unwrap();
            writeln!(
                output,
                "        if (status == KILN_C99_THROWN) goto kiln_unwind;"
            )
            .unwrap();
            writeln!(output, "        if (status != KILN_C99_OK) return status;").unwrap();
            writeln!(output, "        v{v} = v{};", value.0).unwrap();
            writeln!(output, "    }}").unwrap();
        }
        GraphOp::GetComputed { object, key, .. } => {
            writeln!(output, "    {{").unwrap();
            writeln!(
                output,
                "        kiln_val site_stack[2] = {{ v{}, v{} }};",
                object.0, key.0
            )
            .unwrap();
            writeln!(output, "        size_t site_sp = 2;").unwrap();
            writeln!(
                output,
                "        int status = kiln_c99_op_val_get_computed(host, site_stack, &site_sp, depth, thrown);"
            )
            .unwrap();
            writeln!(
                output,
                "        if (status == KILN_C99_THROWN) goto kiln_unwind;"
            )
            .unwrap();
            writeln!(output, "        if (status != KILN_C99_OK) return status;").unwrap();
            writeln!(output, "        v{v} = site_stack[0];").unwrap();
            writeln!(output, "    }}").unwrap();
        }
        GraphOp::SetComputed {
            object, key, value, ..
        }
        | GraphOp::SetComputedStrict {
            object, key, value, ..
        } => {
            let strict_val = if let GraphOp::SetComputedStrict { strict, .. } = op {
                if *strict { 1 } else { 0 }
            } else {
                0
            };
            writeln!(output, "    {{").unwrap();
            writeln!(
                output,
                "        kiln_val site_stack[3] = {{ v{}, v{}, v{} }};",
                object.0, key.0, value.0
            )
            .unwrap();
            writeln!(output, "        size_t site_sp = 3;").unwrap();
            writeln!(
                output,
                "        int status = kiln_c99_op_val_set_computed(host, site_stack, &site_sp, {strict_val}, depth, thrown);"
            )
            .unwrap();
            writeln!(
                output,
                "        if (status == KILN_C99_THROWN) goto kiln_unwind;"
            )
            .unwrap();
            writeln!(output, "        if (status != KILN_C99_OK) return status;").unwrap();
            writeln!(output, "        v{v} = v{};", value.0).unwrap();
            writeln!(output, "    }}").unwrap();
        }
        GraphOp::HostCall { op, args, .. } => {
            let argc = args.len();
            let op_val = *op as u16;
            writeln!(output, "    {{").unwrap();
            writeln!(output, "        kiln_value call_stack[{}];", argc.max(1)).unwrap();
            writeln!(output, "        size_t call_sp = {argc};").unwrap();
            for (i, arg) in args.iter().enumerate() {
                writeln!(
                    output,
                    "        call_stack[{i}] = kiln_val_to_boundary(v{});",
                    arg.0
                )
                .unwrap();
            }
            writeln!(
                output,
                "        int status = kiln_c99_op_host_call(host, call_stack, &call_sp, {op_val}, depth, thrown, \"{diagnostic_site}\", 0, \"HostCall\", NULL);"
            )
            .unwrap();
            writeln!(
                output,
                "        if (status == KILN_C99_THROWN) goto kiln_unwind;"
            )
            .unwrap();
            writeln!(output, "        if (status != KILN_C99_OK) return status;").unwrap();
            writeln!(
                output,
                "        v{v} = kiln_val_from_boundary(call_stack[0]);"
            )
            .unwrap();
            writeln!(output, "    }}").unwrap();
        }
        GraphOp::Phi { .. } => {
            // Phis are resolved by incoming jumps/branches in predecessor blocks.
        }
        _ => {
            // Default fallback for remaining operations
            writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap();
        }
    }
}

fn emit_terminator(
    output: &mut String,
    graph: &FunctionGraph,
    current_block: &BasicBlock,
    terminator: &Terminator,
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
            emit_phi_assignments(output, graph, current_block.id, *target);
            writeln!(output, "    goto kiln_block_{};", target.0).unwrap();
        }
        Terminator::Branch {
            condition,
            true_block,
            false_block,
        } => {
            writeln!(output, "    if (kiln_c99_val_truthy(v{})) {{", condition.0).unwrap();
            emit_phi_assignments(output, graph, current_block.id, *true_block);
            writeln!(output, "        goto kiln_block_{};", true_block.0).unwrap();
            output.push_str("    } else {\n");
            emit_phi_assignments(output, graph, current_block.id, *false_block);
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
) {
    if let Some(target_block) = graph.block(target) {
        for &(phi_vid, ref op, _) in &target_block.operations {
            if let GraphOp::Phi { inputs } = op {
                for &(pred, in_vid) in inputs {
                    if pred == from {
                        writeln!(output, "        v{} = v{};", phi_vid.0, in_vid.0).unwrap();
                    }
                }
            }
        }
    }
}
