use super::OpContext;
use super::{
    binary_code, binary_fast_body, binary_raw_body, status_check, unary_code, unary_fast_body,
};
use crate::graph::{GraphOp, ValueId, as_safe_integer};
use kiln_ir::{BinaryOp, UnaryOp};
use std::fmt::Write;

/// `GraphOp::Unary { op, value } if typed_int`, moved out of `emit_graph_op`'s match verbatim.
pub(super) fn emit_unary_int(
    output: &mut String,
    vid: ValueId,
    op: &UnaryOp,
    value: &ValueId,
    ctx: &OpContext,
) {
    let v = vid.0;
    let ints = ctx.ints;
    let ivs = ctx.ivs;
    let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
    let need_double = ctx.live_numbers.contains(&vid);
    let box_stmt = if need_box {
        if need_double {
            format!(" v{v} = kiln_c99_val_from_double(d{v});")
        } else {
            format!(" v{v} = kiln_c99_val_from_double((double)i{v});")
        }
    } else {
        String::new()
    };
    let copy_box = if need_box {
        format!(" v{v} = v{x};", x = value.0)
    } else {
        String::new()
    };

    let double_assign = if need_double {
        format!(" d{v} = (double)i{v};")
    } else {
        String::new()
    };

    let x = value.0;
    match op {
        UnaryOp::Plus | UnaryOp::ToNumeric => {
            if ints.contains(value) {
                let d_stmt = if need_double {
                    format!(" d{v} = (double)i{x};")
                } else {
                    String::new()
                };
                writeln!(output, "    i{v} = i{x};{d_stmt}{copy_box}").unwrap();
            } else {
                let src_d = if ctx.live_numbers.contains(value) {
                    format!("d{x}")
                } else if ctx.live_ints.contains(value) {
                    format!("(double)i{x}")
                } else {
                    format!("d{x}")
                };
                let d_stmt = if need_double {
                    format!(" d{v} = {src_d};")
                } else {
                    String::new()
                };
                writeln!(output, "   {d_stmt} i{v} = (int64_t){src_d};{copy_box}").unwrap();
            }
        }
        UnaryOp::Increment => {
            if ivs.contains_key(&vid) {
                writeln!(output, "    i{v} = i{x} + 1LL;{double_assign}{box_stmt}").unwrap();
            } else if ints.contains(value) {
                let (d_assign_res, slow_calc) = if need_double {
                    (
                        format!(" d{v} = (double)res;"),
                        format!(
                            " d{v} = kiln_c99_int_slow((double)i{x} + 1.0); i{v} = (int64_t)d{v};"
                        ),
                    )
                } else {
                    (
                        String::new(),
                        format!(
                            " {{ double d_tmp = kiln_c99_int_slow((double)i{x} + 1.0); i{v} = (int64_t)d_tmp; }}"
                        ),
                    )
                };
                writeln!(
                    output,
                    "    {{ int64_t res; if (KILN_LIKELY(!__builtin_add_overflow(i{x}, 1LL, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL)) {{ i{v} = res;{d_assign_res} }} else {{{slow_calc} }}{box_stmt} }}"
                )
                .unwrap();
            } else {
                let src_d = if ctx.live_numbers.contains(value) {
                    format!("d{x}")
                } else if ctx.live_ints.contains(value) {
                    format!("(double)i{x}")
                } else {
                    format!("d{x}")
                };
                let d_stmt = if need_double {
                    format!(" d{v} = {src_d} + 1.0;")
                } else {
                    String::new()
                };
                writeln!(
                    output,
                    "   {d_stmt} i{v} = (int64_t)({src_d} + 1.0);{box_stmt}"
                )
                .unwrap();
            }
        }
        UnaryOp::Decrement => {
            if ivs.contains_key(&vid) {
                writeln!(output, "    i{v} = i{x} - 1LL;{double_assign}{box_stmt}").unwrap();
            } else if ints.contains(value) {
                let (d_assign_res, slow_calc) = if need_double {
                    (
                        format!(" d{v} = (double)res;"),
                        format!(
                            " d{v} = kiln_c99_int_slow((double)i{x} - 1.0); i{v} = (int64_t)d{v};"
                        ),
                    )
                } else {
                    (
                        String::new(),
                        format!(
                            " {{ double d_tmp = kiln_c99_int_slow((double)i{x} - 1.0); i{v} = (int64_t)d_tmp; }}"
                        ),
                    )
                };
                writeln!(
                    output,
                    "    {{ int64_t res; if (KILN_LIKELY(!__builtin_sub_overflow(i{x}, 1LL, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL)) {{ i{v} = res;{d_assign_res} }} else {{{slow_calc} }}{box_stmt} }}"
                )
                .unwrap();
            } else {
                let src_d = if ctx.live_numbers.contains(value) {
                    format!("d{x}")
                } else if ctx.live_ints.contains(value) {
                    format!("(double)i{x}")
                } else {
                    format!("d{x}")
                };
                let d_stmt = if need_double {
                    format!(" d{v} = {src_d} - 1.0;")
                } else {
                    String::new()
                };
                writeln!(
                    output,
                    "   {d_stmt} i{v} = (int64_t)({src_d} - 1.0);{box_stmt}"
                )
                .unwrap();
            }
        }
        UnaryOp::BitwiseNot => {
            if ints.contains(value) {
                writeln!(
                    output,
                    "    i{v} = (int64_t)(~(int32_t)i{x});{double_assign}{box_stmt}"
                )
                .unwrap();
            } else {
                let sync_d = if need_double {
                    format!(" memcpy(&d{v}, &v{v}, sizeof(d{v})); i{v} = (int64_t)d{v};")
                } else {
                    format!(
                        " {{ double tmp; memcpy(&tmp, &v{v}, sizeof(tmp)); i{v} = (int64_t)tmp; }}"
                    )
                };
                writeln!(
                    output,
                    "    v{v} = kiln_c99_val_unary_fast(5, v{x});{sync_d}"
                )
                .unwrap();
            }
        }
        _ => {
            let src_d = if ctx.live_numbers.contains(value) {
                format!("d{x}")
            } else if ctx.live_ints.contains(value) {
                format!("(double)i{x}")
            } else {
                format!("d{x}")
            };
            let fast = unary_fast_body(*op)
                .expect("a proven-int unary has a numeric body")
                .replace("value", &format!("v{x}"))
                .replace("kiln_c99_val_from_double(", "(");
            let expr = fast.replace('a', &src_d);
            let d_stmt = if need_double {
                format!(" d{v} = {expr};")
            } else {
                String::new()
            };
            writeln!(output, "   {d_stmt} i{v} = (int64_t)({expr});{box_stmt}").unwrap();
        }
    }
}

/// `GraphOp::Unary { op, value } if typed`, moved out of `emit_graph_op`'s match verbatim.
pub(super) fn emit_unary_typed(
    output: &mut String,
    vid: ValueId,
    op: &UnaryOp,
    value: &ValueId,
    ctx: &OpContext,
) {
    let v = vid.0;
    let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
    let need_double = ctx.live_numbers.contains(&vid);
    let box_stmt = if need_box {
        if need_double {
            format!(" v{v} = kiln_c99_val_from_double(d{v});")
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    let copy_box = if need_box {
        format!(" v{v} = v{x};", x = value.0)
    } else {
        String::new()
    };

    // Proven number in, proven number out: the double is computed directly.
    let x = value.0;
    let src_d = if ctx.live_numbers.contains(value) {
        format!("d{x}")
    } else if ctx.live_ints.contains(value) {
        format!("(double)i{x}")
    } else {
        format!("d{x}")
    };
    let fast = unary_fast_body(*op)
        .expect("a proven-number unary has a numeric body")
        .replace("value", &format!("v{x}"))
        .replace("kiln_c99_val_from_double(", "(");
    match op {
        UnaryOp::Plus | UnaryOp::ToNumeric => {
            if need_double {
                writeln!(output, "    d{v} = {src_d};{copy_box}").unwrap();
            } else if need_box {
                writeln!(output, "    v{v} = v{x};").unwrap();
            }
        }
        UnaryOp::BitwiseNot => {
            if need_double {
                writeln!(
                    output,
                    "    v{v} = kiln_c99_val_unary_fast(5, v{x}); memcpy(&d{v}, &v{v}, sizeof(d{v}));{box_stmt}"
                )
                .unwrap();
            } else {
                writeln!(output, "    v{v} = kiln_c99_val_unary_fast(5, v{x});").unwrap();
            }
        }
        _ => {
            let expr = fast.replace('a', &src_d);
            if need_double {
                writeln!(output, "    d{v} = {expr};{box_stmt}").unwrap();
            } else if need_box {
                writeln!(output, "    v{v} = kiln_c99_val_from_double({expr});").unwrap();
            }
        }
    }
}

/// `GraphOp::Unary { op, value }`, the plain fallback, moved out of `emit_graph_op`'s match
/// verbatim.
pub(super) fn emit_unary_boxed(
    output: &mut String,
    vid: ValueId,
    op: &UnaryOp,
    value: &ValueId,
    ctx: &OpContext,
) {
    let v = vid.0;
    let diagnostic_site = ctx.diagnostic_site;
    let scalar_proto = ctx.scalar_proto;
    let unwind = ctx.unwind;
    let (pc, label) = ctx
        .origins
        .get(&vid)
        .map(|o| (o.pc, o.label.as_str()))
        .unwrap_or((0, "Op"));

    let x = value.0;
    let code = unary_code(*op);
    if ctx.plan.generic_only && !matches!(op, UnaryOp::Void | UnaryOp::Not | UnaryOp::TypeOf) {
        let check = status_check(diagnostic_site, pc, label, unwind);
        if scalar_proto {
            writeln!(output, "    v{v} = kiln_c99_val_unary_fast({code}, v{x});").unwrap();
        } else {
            writeln!(output, "    {{ kiln_value ans; int status = kiln_c99_unary_checked({code}, host, kiln_val_to_boundary(v{x}), &ans, depth, thrown); {check} v{v} = kiln_val_from_boundary(ans); }}").unwrap();
        }
        return;
    }
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
            // and takes the pure helper.
            let slow = if scalar_proto {
                format!("v{v} = kiln_c99_val_unary_fast({code}, v{x});")
            } else {
                let check = status_check(diagnostic_site, pc, label, unwind);
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

/// `GraphOp::Binary { op, left, right } if ints.contains(left) && ints.contains(right) && ..`,
/// moved out of `emit_graph_op`'s match verbatim.
pub(super) fn emit_binary_ints(
    output: &mut String,
    vid: ValueId,
    op: &BinaryOp,
    left: &ValueId,
    right: &ValueId,
    ctx: &OpContext,
) {
    let v = vid.0;
    let defs = ctx.defs;
    let ivs = ctx.ivs;
    let typed_int = ctx.ints.contains(&vid);
    let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
    let need_double = ctx.live_numbers.contains(&vid);
    let box_stmt = if need_box {
        if need_double {
            format!(" v{v} = kiln_c99_val_from_double(d{v});")
        } else {
            format!(" v{v} = kiln_c99_val_from_double((double)i{v});")
        }
    } else {
        String::new()
    };
    let double_assign = if need_double {
        format!(" d{v} = (double)i{v};")
    } else {
        String::new()
    };

    let (l, r) = (left.0, right.0);
    let need_bool = ctx.booleans.contains(&vid);
    let sym = match op {
        BinaryOp::StrictEqual | BinaryOp::LooseEqual => Some("=="),
        BinaryOp::StrictNotEqual | BinaryOp::LooseNotEqual => Some("!="),
        BinaryOp::LessThan => Some("<"),
        BinaryOp::LessThanOrEqual => Some("<="),
        BinaryOp::GreaterThan => Some(">"),
        BinaryOp::GreaterThanOrEqual => Some(">="),
        _ => None,
    };
    if let Some(sym) = sym {
        if need_bool && need_box {
            writeln!(
                output,
                "    b{v} = (i{l} {sym} i{r}); v{v} = b{v} ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
            )
            .unwrap();
        } else if need_bool {
            writeln!(output, "    b{v} = (i{l} {sym} i{r});").unwrap();
        } else if need_box {
            writeln!(
                output,
                "    v{v} = (i{l} {sym} i{r}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
            )
            .unwrap();
        }
        return;
    }
    match op {
        BinaryOp::BitwiseAnd => {
            writeln!(
                output,
                "    i{v} = (int64_t)((int32_t)i{l} & (int32_t)i{r});{double_assign}{box_stmt}"
            )
            .unwrap();
        }
        BinaryOp::BitwiseOr => {
            writeln!(
                output,
                "    i{v} = (int64_t)((int32_t)i{l} | (int32_t)i{r});{double_assign}{box_stmt}"
            )
            .unwrap();
        }
        BinaryOp::BitwiseXor => {
            writeln!(
                output,
                "    i{v} = (int64_t)((int32_t)i{l} ^ (int32_t)i{r});{double_assign}{box_stmt}"
            )
            .unwrap();
        }
        BinaryOp::ShiftLeft => {
            writeln!(output, "    i{v} = (int64_t)(int32_t)((uint32_t)i{l} << ((uint32_t)i{r} & 31));{double_assign}{box_stmt}").unwrap();
        }
        BinaryOp::ShiftRight => {
            writeln!(output, "    i{v} = (int64_t)((int32_t)i{l} >> ((uint32_t)i{r} & 31));{double_assign}{box_stmt}").unwrap();
        }
        BinaryOp::UnsignedShiftRight => {
            writeln!(output, "    i{v} = (int64_t)((uint32_t)i{l} >> ((uint32_t)i{r} & 31));{double_assign}{box_stmt}").unwrap();
        }
        BinaryOp::Remainder => {
            let is_const_nonzero = defs.get(right).is_some_and(|op| match op {
                GraphOp::Const { constant, .. } => {
                    as_safe_integer(constant).is_some_and(|k| k != 0)
                }
                _ => false,
            });
            let non_negative = ivs.get(left).is_some_and(|iv| iv.non_negative);
            if is_const_nonzero {
                if non_negative {
                    writeln!(output, "    i{v} = i{l} % i{r};{double_assign}{box_stmt}").unwrap();
                } else {
                    let d_rem = if need_double {
                        format!(" d{v} = (i{v} == 0 && i{l} < 0) ? -0.0 : (double)i{v};")
                    } else {
                        String::new()
                    };
                    writeln!(output, "    i{v} = i{l} % i{r};{d_rem}{box_stmt}").unwrap();
                }
            } else {
                let d_rem = if need_double {
                    format!(" d{v} = (i{v} == 0 && i{l} < 0) ? -0.0 : (double)i{v};")
                } else {
                    String::new()
                };
                let d_nan = if need_double {
                    format!(" d{v} = NAN;")
                } else {
                    String::new()
                };
                writeln!(output, "    if (KILN_LIKELY(i{r} != 0)) {{ i{v} = i{l} % i{r};{d_rem} }} else {{{d_nan} i{v} = 0; }}{box_stmt}").unwrap();
            }
        }
        BinaryOp::Add => {
            if ivs.contains_key(&vid) {
                writeln!(output, "    i{v} = i{l} + i{r};{double_assign}{box_stmt}").unwrap();
            } else if typed_int {
                let (d_assign_res, slow_calc) = if need_double {
                    (
                        format!(" d{v} = (double)res;"),
                        format!(
                            " d{v} = kiln_c99_int_slow((double)i{l} + (double)i{r}); i{v} = isfinite(d{v}) ? (int64_t)d{v} : 0;"
                        ),
                    )
                } else {
                    (
                        String::new(),
                        format!(
                            " {{ double d_tmp = kiln_c99_int_slow((double)i{l} + (double)i{r}); i{v} = isfinite(d_tmp) ? (int64_t)d_tmp : 0; }}"
                        ),
                    )
                };
                writeln!(
                    output,
                    "    {{ int64_t res; if (KILN_LIKELY(!__builtin_add_overflow(i{l}, i{r}, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL)) {{ i{v} = res;{d_assign_res} }} else {{{slow_calc} }}{box_stmt} }}"
                ).unwrap();
            } else {
                writeln!(output, "    d{v} = (double)i{l} + (double)i{r};{box_stmt}").unwrap();
            }
        }
        BinaryOp::Subtract => {
            if ivs.contains_key(&vid) {
                writeln!(output, "    i{v} = i{l} - i{r};{double_assign}{box_stmt}").unwrap();
            } else if typed_int {
                let (d_assign_res, slow_calc) = if need_double {
                    (
                        format!(" d{v} = (double)res;"),
                        format!(
                            " d{v} = kiln_c99_int_slow((double)i{l} - (double)i{r}); i{v} = isfinite(d{v}) ? (int64_t)d{v} : 0;"
                        ),
                    )
                } else {
                    (
                        String::new(),
                        format!(
                            " {{ double d_tmp = kiln_c99_int_slow((double)i{l} - (double)i{r}); i{v} = isfinite(d_tmp) ? (int64_t)d_tmp : 0; }}"
                        ),
                    )
                };
                writeln!(
                    output,
                    "    {{ int64_t res; if (KILN_LIKELY(!__builtin_sub_overflow(i{l}, i{r}, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL)) {{ i{v} = res;{d_assign_res} }} else {{{slow_calc} }}{box_stmt} }}"
                ).unwrap();
            } else {
                writeln!(output, "    d{v} = (double)i{l} - (double)i{r};{box_stmt}").unwrap();
            }
        }
        BinaryOp::Multiply => {
            if typed_int {
                let (d_assign_res, slow_calc) = if need_double {
                    (
                        format!(" d{v} = (double)res;"),
                        format!(
                            " d{v} = (double)i{l} * (double)i{r}; i{v} = isfinite(d{v}) ? (int64_t)d{v} : 0;"
                        ),
                    )
                } else {
                    (
                        String::new(),
                        format!(
                            " {{ double d_tmp = (double)i{l} * (double)i{r}; i{v} = isfinite(d_tmp) ? (int64_t)d_tmp : 0; }}"
                        ),
                    )
                };
                writeln!(
                    output,
                    "    {{ int64_t res; if (KILN_LIKELY(!__builtin_mul_overflow(i{l}, i{r}, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL && (res != 0 || (i{l} >= 0 && i{r} >= 0) || (i{l} < 0 && i{r} < 0)))) {{ i{v} = res;{d_assign_res} }} else {{{slow_calc} }}{box_stmt} }}"
                ).unwrap();
            } else {
                writeln!(
                    output,
                    "    {{ int64_t res; if (KILN_LIKELY(!__builtin_mul_overflow(i{l}, i{r}, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL && (res != 0 || (i{l} >= 0 && i{r} >= 0) || (i{l} < 0 && i{r} < 0)))) {{ d{v} = (double)res; }} else {{ d{v} = (double)i{l} * (double)i{r}; }}{box_stmt} }}"
                ).unwrap();
            }
        }
        BinaryOp::Divide => {
            writeln!(output, "    d{v} = (double)i{l} / (double)i{r};{box_stmt}").unwrap();
            if typed_int {
                writeln!(output, "    i{v} = isfinite(d{v}) ? (int64_t)d{v} : 0;").unwrap();
            }
        }
        BinaryOp::Exponentiate => {
            writeln!(
                output,
                "    d{v} = pow((double)i{l}, (double)i{r});{box_stmt}"
            )
            .unwrap();
            if typed_int {
                writeln!(output, "    i{v} = isfinite(d{v}) ? (int64_t)d{v} : 0;").unwrap();
            }
        }
        BinaryOp::In | BinaryOp::InstanceOf => {
            unreachable!("`in` and `instanceof` take the generic lowering")
        }
        _ => unreachable!("comparisons handled above"),
    }
}

/// `GraphOp::Binary { op, left, right } if numbers.contains(left) && numbers.contains(right) &&
/// ..`, moved out of `emit_graph_op`'s match verbatim.
pub(super) fn emit_binary_numbers(
    output: &mut String,
    vid: ValueId,
    op: &BinaryOp,
    left: &ValueId,
    right: &ValueId,
    ctx: &OpContext,
) {
    let v = vid.0;
    let typed_int = ctx.live_ints.contains(&vid);
    let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
    let need_double = ctx.live_numbers.contains(&vid);
    let box_stmt = if need_box {
        if need_double {
            format!(" v{v} = kiln_c99_val_from_double(d{v});")
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Both operands proven numbers: no tag test, no memcpy. A comparison yields the
    // boolean directly; arithmetic computes the double twin and boxes it. `in` and
    // `instanceof` on two numbers are a `TypeError` the generic lowering throws.
    let (l, r) = (left.0, right.0);
    let a = if ctx.live_numbers.contains(left) {
        format!("d{l}")
    } else if ctx.live_ints.contains(left) {
        format!("(double)i{l}")
    } else {
        format!("d{l}")
    };
    let b = if ctx.live_numbers.contains(right) {
        format!("d{r}")
    } else if ctx.live_ints.contains(right) {
        format!("(double)i{r}")
    } else {
        format!("d{r}")
    };
    let need_bool = ctx.booleans.contains(&vid);
    let sym = match op {
        BinaryOp::StrictEqual | BinaryOp::LooseEqual => Some("=="),
        BinaryOp::StrictNotEqual | BinaryOp::LooseNotEqual => Some("!="),
        BinaryOp::LessThan => Some("<"),
        BinaryOp::LessThanOrEqual => Some("<="),
        BinaryOp::GreaterThan => Some(">"),
        BinaryOp::GreaterThanOrEqual => Some(">="),
        _ => None,
    };
    if let Some(sym) = sym {
        if need_bool && need_box {
            writeln!(
                output,
                "    b{v} = ({a} {sym} {b}); v{v} = b{v} ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
            )
            .unwrap();
        } else if need_bool {
            writeln!(output, "    b{v} = ({a} {sym} {b});").unwrap();
        } else if need_box {
            writeln!(
                output,
                "    v{v} = ({a} {sym} {b}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
            )
            .unwrap();
        }
        return;
    }
    let emit_arith = |output: &mut String, expr: &str| {
        if need_double {
            writeln!(output, "    d{v} = {expr};{box_stmt}").unwrap();
            if typed_int {
                writeln!(output, "    i{v} = (int64_t)d{v};").unwrap();
            }
        } else {
            if need_box {
                writeln!(output, "    v{v} = kiln_c99_val_from_double({expr});").unwrap();
            }
            if typed_int {
                writeln!(output, "    i{v} = (int64_t)({expr});").unwrap();
            }
        }
    };
    match op {
        BinaryOp::Add => emit_arith(output, &format!("{a} + {b}")),
        BinaryOp::Subtract => emit_arith(output, &format!("{a} - {b}")),
        BinaryOp::Multiply => emit_arith(output, &format!("{a} * {b}")),
        BinaryOp::Divide => emit_arith(output, &format!("{a} / {b}")),
        BinaryOp::Exponentiate => emit_arith(output, &format!("pow({a}, {b})")),
        BinaryOp::Remainder => emit_arith(
            output,
            &format!(
                "({b} > 0.0 && {b} <= 9007199254740992.0 && {a} >= 0.0 && {a} <= 9007199254740992.0 && {a} == (double)(int64_t){a} && {b} == (double)(int64_t){b}) ? (double)((int64_t){a} % (int64_t){b}) : fmod({a}, {b})"
            ),
        ),
        BinaryOp::BitwiseAnd
        | BinaryOp::BitwiseOr
        | BinaryOp::BitwiseXor
        | BinaryOp::ShiftLeft
        | BinaryOp::ShiftRight
        | BinaryOp::UnsignedShiftRight => {
            let sync_d = if need_double {
                format!(" memcpy(&d{v}, &v{v}, sizeof(d{v}));")
            } else {
                String::new()
            };
            let sync_i = if typed_int {
                if need_double {
                    format!(" i{v} = (int64_t)d{v};")
                } else {
                    format!(
                        " {{ double tmp_{v}; memcpy(&tmp_{v}, &v{v}, sizeof(tmp_{v})); i{v} = (int64_t)tmp_{v}; }}"
                    )
                }
            } else {
                String::new()
            };
            writeln!(
                output,
                "    v{v} = kiln_c99_val_binary_fast({}, v{l}, v{r});{sync_d}{sync_i}",
                binary_code(*op)
            )
            .unwrap();
        }
        BinaryOp::In | BinaryOp::InstanceOf => {
            unreachable!("`in` and `instanceof` take the generic lowering")
        }
        _ => unreachable!("comparisons handled above"),
    }
}

/// `GraphOp::Binary { op: BinaryOp::In, left, right }`, moved out of `emit_graph_op`'s match
/// verbatim.
pub(super) fn emit_binary_in(
    output: &mut String,
    vid: ValueId,
    left: &ValueId,
    right: &ValueId,
    ctx: &OpContext,
) {
    let v = vid.0;
    let unwind = ctx.unwind;

    writeln!(
                output,
                "    {{ kiln_value container = kiln_val_to_boundary(v{r}); int answer; if (!kiln_c99_is_reference(container) && container.tag != KILN_VALUE_DATE) {{ (void)kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, \"Cannot use 'in' to search for a property in a non-object\"); {unwind} }} int status = kiln_c99_has_computed(host, container, kiln_val_to_boundary(v{l}), &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; v{v} = answer ? KILN_VAL_TRUE : KILN_VAL_FALSE; }}",
                l = left.0,
                r = right.0
            )
            .unwrap();
}

/// `GraphOp::Binary { op: BinaryOp::InstanceOf, left, right }`, moved out of `emit_graph_op`'s
/// match verbatim.
pub(super) fn emit_binary_instance_of(
    output: &mut String,
    vid: ValueId,
    left: &ValueId,
    right: &ValueId,
    _ctx: &OpContext,
) {
    let v = vid.0;

    writeln!(
                output,
                "    {{ kiln_value constructor = kiln_val_to_boundary(v{r}); int answer; int status = kiln_c99_instance_of(kiln_val_to_boundary(v{l}), constructor, &answer); if (status != KILN_C99_OK) return status; v{v} = answer ? KILN_VAL_TRUE : KILN_VAL_FALSE; }}",
                l = left.0,
                r = right.0
            )
            .unwrap();
}

/// `GraphOp::Binary { op, left, right }`, the plain fallback, moved out of `emit_graph_op`'s
/// match verbatim.
pub(super) fn emit_binary_boxed(
    output: &mut String,
    vid: ValueId,
    op: &BinaryOp,
    left: &ValueId,
    right: &ValueId,
    ctx: &OpContext,
) {
    let v = vid.0;
    let diagnostic_site = ctx.diagnostic_site;
    let scalar_proto = ctx.scalar_proto;
    let unwind = ctx.unwind;
    let typed = ctx.numbers.contains(&vid);
    let typed_int = ctx.ints.contains(&vid);
    let (pc, label) = ctx
        .origins
        .get(&vid)
        .map(|o| (o.pc, o.label.as_str()))
        .unwrap_or((0, "Op"));

    let (l, r) = (left.0, right.0);
    let code = binary_code(*op);
    // The slow path runs the program's own conversions and can throw; the pure helper
    // is only for a body that cannot hold a reference (`scalar_proto`).
    let slow = if scalar_proto {
        format!("v{v} = kiln_c99_val_binary_fast({code}, v{l}, v{r});")
    } else if crate::compact_diagnostic_site(diagnostic_site) {
        format!("KU_BINARY(v{v}, {code}, v{l}, v{r}, {pc}, \"{label}\", {unwind});")
    } else {
        let check = status_check(diagnostic_site, pc, label, unwind);
        format!(
            "int status = KILN_C99_OK; v{v} = kiln_c99_val_binary({code}, host, v{l}, v{r}, &status, depth, thrown); {check}"
        )
    };
    if ctx.plan.generic_only {
        writeln!(output, "    {{ {slow} }}").unwrap();
        return;
    }
    let fast = binary_fast_body(*op).map(|f| {
        f.replace("LEFT", &format!("v{l}"))
            .replace("RIGHT", &format!("v{r}"))
    });
    // A result proven a number -- by a restart, in a restartable body -- is computed
    // as its twin and boxed from it, so a box nothing reads costs nothing: the NaN
    // canonicalisation is the box's, not the arithmetic's. The slow path, where it
    // survives, keeps the twin in step.
    let raw = if typed && !typed_int {
        binary_raw_body(*op)
    } else {
        None
    };
    // An operand already proven a number is not tested again, and its twin is read rather than
    // copied back out of its box. That matters most where it is loop-carried: an accumulator
    // tested and unpacked on every pass moves between the integer and floating registers twice
    // a turn, and the moves are the loop's critical path rather than the arithmetic.
    let proven_operand =
        |operand: &ValueId| ctx.numbers.contains(operand) && !ctx.ints.contains(operand);
    let (l_proven, r_proven) = (proven_operand(left), proven_operand(right));
    let mut tests: Vec<String> = Vec::new();
    if !l_proven {
        tests.push(format!("KILN_VAL_IS_NUMBER(v{l})"));
    }
    if !r_proven {
        tests.push(format!("KILN_VAL_IS_NUMBER(v{r})"));
    }
    let guard = if tests.is_empty() {
        "1".to_owned()
    } else {
        tests.join(" && ")
    };
    let l_twin = ctx.live_numbers.contains(left);
    let r_twin = ctx.live_numbers.contains(right);
    let bind = format!(
        "{} {}",
        if l_proven && l_twin {
            format!("double a = d{l};")
        } else if ctx.live_ints.contains(left) {
            format!("double a = (double)i{l};")
        } else {
            format!("double a; memcpy(&a, &v{l}, sizeof(a));")
        },
        if r_proven && r_twin {
            format!("double b = d{r};")
        } else if ctx.live_ints.contains(right) {
            format!("double b = (double)i{r};")
        } else {
            format!("double b; memcpy(&b, &v{r}, sizeof(b));")
        }
    );
    let live_d = ctx.live_numbers.contains(&vid);
    let live_i = ctx.live_ints.contains(&vid);
    match (fast, raw) {
        (Some(_), Some(raw)) => {
            if live_d {
                writeln!(
                    output,
                    "    if (KILN_LIKELY({guard})) {{ {bind} d{v} = {raw}; v{v} = kiln_c99_val_from_double(d{v}); }} else {{ {slow} memcpy(&d{v}, &v{v}, sizeof(d{v})); }}"
                )
                .unwrap();
            } else {
                writeln!(
                    output,
                    "    if (KILN_LIKELY({guard})) {{ {bind} v{v} = kiln_c99_val_from_double({raw}); }} else {{ {slow} }}"
                )
                .unwrap();
            }
        }
        (Some(fast), None) => writeln!(
            output,
            "    if (KILN_LIKELY({guard})) {{ {bind} v{v} = {fast}; }} else {{ {slow} }}"
        )
        .unwrap(),
        (None, _) => writeln!(output, "    {{ {slow} }}").unwrap(),
    }
    if live_d {
        if typed_int {
            writeln!(
                output,
                "    if (KILN_VAL_IS_NUMBER(v{v})) {{ memcpy(&d{v}, &v{v}, sizeof(d{v})); }}"
            )
            .unwrap();
        } else if typed && raw.is_none() {
            // Proven a number by a restart: the guarded form ran, so the twin is the bits.
            writeln!(output, "    memcpy(&d{v}, &v{v}, sizeof(d{v}));").unwrap();
        }
    }
    if live_i && typed_int {
        if live_d {
            writeln!(
                output,
                "    if (KILN_VAL_IS_NUMBER(v{v})) {{ i{v} = (int64_t)d{v}; }}"
            )
            .unwrap();
        } else {
            writeln!(output, "    if (KILN_VAL_IS_NUMBER(v{v})) {{ double tmp_{v}; memcpy(&tmp_{v}, &v{v}, sizeof(tmp_{v})); i{v} = (int64_t)tmp_{v}; }}").unwrap();
        }
    }
}
