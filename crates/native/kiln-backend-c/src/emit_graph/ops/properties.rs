use super::OpContext;
use super::status_check;
use crate::emit_graph::EntryKind;
use crate::graph::{GraphOp, ValueId, resolve_const_string};
use kiln_ir::Constant;
use std::fmt::Write;

/// `GraphOp::GetProperty { object, name, .. }`, moved out of `emit_graph_op`'s match verbatim.
pub(super) fn emit_get_property(
    output: &mut String,
    vid: ValueId,
    object: &ValueId,
    name: &u32,
    ctx: &OpContext,
) {
    let v = vid.0;
    let graph = ctx.graph;
    let program = ctx.program;
    let symbol = ctx.symbol;
    let diagnostic_site = ctx.diagnostic_site;
    let unwind = ctx.unwind;
    let self_index = ctx.self_index;
    let this_site = ctx.this_site;
    let spec = ctx.spec;
    let typed = ctx.live_numbers.contains(&vid);
    let typed_int = ctx.live_ints.contains(&vid);
    let (pc, label) = ctx
        .origins
        .get(&vid)
        .map(|o| (o.pc, o.label.as_str()))
        .unwrap_or((0, "Op"));
    if let Some(&root) = graph.scalar_alias_map.get(object)
        && let Some(scalar_obj) = graph.scalar_objects.get(&root)
    {
        let alloc = root.0;
        // A field of a sunk literal guessed to be a number: checked, and the literal rebuilt for
        // the generic clone when the guess was wrong.
        let checked = spec.exits && spec.fields.contains(&vid);
        if let Some(&field_idx) = scalar_obj.field_by_name_index.get(name) {
            writeln!(output, "    v{v} = v{alloc}_f_{field_idx};").unwrap();
            if checked {
                let invariant = crate::prelude::lookup_speculation_invariant("spread_field_number");
                writeln!(
                    output,
                    "    /* Speculation exit [{}]: {} */ if (KILN_UNLIKELY(!KILN_VAL_IS_NUMBER(v{v}))) {{ {} goto kiln_generic_resume_{v}; }}",
                    invariant.name,
                    invariant.language_rule,
                    super::materialize_all_sunk(graph, symbol, ctx.defs, ctx.array_slots)
                )
                .unwrap();
            }
            if typed {
                writeln!(output, "    d{v} = d{alloc}_f_{field_idx};").unwrap();
            }
            if typed_int {
                writeln!(output, "    i{v} = i{alloc}_f_{field_idx};").unwrap();
            }
        } else {
            if checked {
                writeln!(
                    output,
                    "    {} goto kiln_generic_resume_{v};",
                    super::materialize_all_sunk(graph, symbol, ctx.defs, ctx.array_slots)
                )
                .unwrap();
            }
            writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap();
            if typed {
                writeln!(output, "    d{v} = 0.0;").unwrap();
            }
            if typed_int {
                writeln!(output, "    i{v} = 0;").unwrap();
            }
        }
        return;
    }
    let key_str = match program.constants.get(*name as usize) {
        Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
        _ => String::new(),
    };
    let key_len = key_str.len();
    let check = status_check(diagnostic_site, pc, label, unwind);
    if ctx.plan.generic_only {
        let typed_sync = if typed_int && typed {
            format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v})); i{v} = (int64_t)d{v};")
        } else if typed_int {
            format!(
                "\n    {{ double tmp_{v}; memcpy(&tmp_{v}, &v{v}, sizeof(tmp_{v})); i{v} = (int64_t)tmp_{v}; }}"
            )
        } else if typed {
            format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));")
        } else {
            String::new()
        };
        if crate::compact_diagnostic_site(diagnostic_site) {
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; KU_READ_PROPERTY(v{v}, v{o}, {symbol}_key_{name}, {key_len}, &site, {pc}, \"{label}\", {unwind});{typed_sync} }}",
                o = object.0,
            )
            .unwrap();
        } else {
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; int status = kiln_c99_val_read_property_key(host, v{o}, {symbol}_key_{name}, depth, thrown, &site, &v{v}); if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_property_refusal(kiln_val_to_boundary(v{o})); {check} }}{typed_sync}",
                o = object.0,
            )
            .unwrap();
        }
        return;
    }
    let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
    if key_str == "length" && spec.arrays.contains(object) {
        let box_stmt = if need_box {
            format!(
                " v{v} = kiln_c99_val_from_double((double)kiln_spec_{o}_arr->count);",
                o = object.0,
                v = v
            )
        } else {
            String::new()
        };
        let int_stmt = if typed_int {
            format!(
                " i{v} = (int64_t)kiln_spec_{o}_arr->count;",
                o = object.0,
                v = v
            )
        } else {
            String::new()
        };
        let double_stmt = if typed {
            format!(
                " d{v} = (double)kiln_spec_{o}_arr->count;",
                o = object.0,
                v = v
            )
        } else {
            String::new()
        };
        writeln!(output, "    {{ {int_stmt}{double_stmt}{box_stmt} }}").unwrap();
        return;
    }
    let length_check = if key_str == "length" {
        let box_stmt = if need_box {
            format!(" v{v} = kiln_c99_val_from_double((double)((const kiln_array *)ptr)->count);")
        } else {
            String::new()
        };
        let int_stmt = if typed_int {
            format!(" i{v} = (int64_t)((const kiln_array *)ptr)->count;")
        } else {
            String::new()
        };
        let double_stmt = if typed {
            format!(" d{v} = (double)((const kiln_array *)ptr)->count;")
        } else {
            String::new()
        };
        let ta_box_stmt = if need_box {
            format!(" v{v} = kiln_c99_val_from_double((double)view->length);")
        } else {
            String::new()
        };
        let ta_int_stmt = if typed_int {
            format!(" i{v} = (int64_t)view->length;")
        } else {
            String::new()
        };
        let ta_double_stmt = if typed {
            format!(" d{v} = (double)view->length;")
        } else {
            String::new()
        };
        let str_int_stmt = if typed_int {
            format!(" i{v} = (int64_t)str->unit_len;")
        } else {
            String::new()
        };
        let str_double_stmt = if typed {
            format!(" d{v} = (double)str->unit_len;")
        } else {
            String::new()
        };
        let str_box_stmt = if need_box {
            format!(" v{v} = kiln_c99_val_from_double((double)str->unit_len);")
        } else {
            String::new()
        };
        format!(
            "if (KILN_LIKELY((v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         uint32_t kind = kiln_c99_gc_payload_kind(ptr); \
                         if (KILN_LIKELY(kind == KILN_GC_ARRAY)) {{ \
                             {int_stmt}{double_stmt}{box_stmt} \
                             handled = 1; \
                         }} else if (kind == KILN_GC_TYPED_ARRAY && \
                                    KILN_LIKELY(!((const kiln_typed_array *)ptr)->buffer->detached)) {{ \
                             const kiln_typed_array *view = (const kiln_typed_array *)ptr; \
                             {ta_int_stmt}{ta_double_stmt}{ta_box_stmt} \
                             handled = 1; \
                         }} \
                     }} else if (KILN_VAL_IS_STRING(v{o})) {{ \
                         const kiln_string *str = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         {str_int_stmt}{str_double_stmt}{str_box_stmt} \
                         handled = 1; \
                     }} ",
            o = object.0
        )
    } else {
        String::new()
    };
    let size_check = if key_str == "size" {
        let size_int_stmt = if typed_int {
            format!(" i{v} = (int64_t)col->live;")
        } else {
            String::new()
        };
        let size_double_stmt = if typed {
            format!(" d{v} = (double)col->live;")
        } else {
            String::new()
        };
        let size_box_stmt = if need_box {
            format!(" v{v} = kiln_c99_val_from_double((double)col->live);")
        } else {
            String::new()
        };
        format!(
            "if ((v{o} & KILN_TAG_MASK) == KILN_TAG_OBJECT_PREFIX || (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {{ \
                             const kiln_collection *col = (const kiln_collection *)ptr; \
                             if (!col->is_weak) {{ \
                                 {size_int_stmt}{size_double_stmt}{size_box_stmt} \
                                 handled = 1; \
                             }} \
                         }} \
                     }} ",
            o = object.0
        )
    } else {
        String::new()
    };
    // A read of `this`: the function's receiver-shape cache answers first, one indexed
    // load and no compare, because the shape was matched once at entry. The boxed
    // clone, whose slow path is the one that learns, fills the row afterwards.
    // A read of `this`: the function's receiver-shape cache answers with one indexed
    // load when the receiver's shape has a row, which was filled whole from the shape.
    let is_typed_entry = matches!(ctx.kind, EntryKind::Typed { .. });
    let (receiver_tier, receiver_learn) = match (this_site, self_index) {
        (Some(_), Some(_)) if ctx.this_copy.is_some() => {
            let prev = ctx.this_copy.unwrap().0;
            let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
            if is_typed_entry && typed {
                let box_stmt = if need_box {
                    format!(" v{v} = v{prev};")
                } else {
                    String::new()
                };
                (
                    format!("if (recv_row != NULL) {{ d{v} = d{prev};{box_stmt} handled = 1; }} "),
                    String::new(),
                )
            } else {
                (
                    format!("if (recv_row != NULL) {{ v{v} = v{prev}; handled = 1; }} "),
                    String::new(),
                )
            }
        }
        (Some(i), Some(_)) => {
            let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
            if is_typed_entry && typed {
                let box_stmt_inline = if need_box {
                    format!(" v{v} = recv_obj->inline_slots[recv_row[{i}]];")
                } else {
                    String::new()
                };
                let box_stmt_heap = if need_box {
                    format!(" v{v} = recv_obj->slots[recv_row[{i}]];")
                } else {
                    String::new()
                };
                (
                    format!(
                        "if (recv_row != NULL) {{ if (recv_row[{i}] < KILN_C99_INLINE_SLOTS) {{ memcpy(&d{v}, &recv_obj->inline_slots[recv_row[{i}]], sizeof(double));{box_stmt_inline} }} else {{ memcpy(&d{v}, &recv_obj->slots[recv_row[{i}]], sizeof(double));{box_stmt_heap} }} handled = 1; }} "
                    ),
                    String::new(),
                )
            } else {
                (
                    format!(
                        "if (recv_row != NULL) {{ v{v} = recv_row[{i}] < KILN_C99_INLINE_SLOTS ? recv_obj->inline_slots[recv_row[{i}]] : recv_obj->slots[recv_row[{i}]]; handled = 1; }} "
                    ),
                    String::new(),
                )
            }
        }
        _ => (String::new(), String::new()),
    };
    if is_typed_entry && this_site.is_some() {
        let prev = ctx.this_copy.map(|c| c.0);
        let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
        let int_sync = if typed_int {
            format!(" i{v} = (int64_t)d{v};")
        } else {
            String::new()
        };
        match (this_site, prev) {
            (Some(_), Some(prev_vid)) => {
                let box_stmt = if need_box {
                    format!(" v{v} = v{prev_vid};")
                } else {
                    String::new()
                };
                if typed {
                    writeln!(output, "    d{v} = d{prev_vid};{box_stmt}{int_sync}").unwrap();
                } else {
                    writeln!(output, "    v{v} = v{prev_vid};{int_sync}").unwrap();
                }
            }
            (Some(col_idx), None) => {
                if typed {
                    let box_stmt_inline = if need_box {
                        format!(" v{v} = recv_obj->inline_slots[recv_row[{col_idx}]];")
                    } else {
                        String::new()
                    };
                    let box_stmt_heap = if need_box {
                        format!(" v{v} = recv_obj->slots[recv_row[{col_idx}]];")
                    } else {
                        String::new()
                    };
                    writeln!(
                        output,
                        "    if (recv_row[{col_idx}] < KILN_C99_INLINE_SLOTS) {{ memcpy(&d{v}, &recv_obj->inline_slots[recv_row[{col_idx}]], sizeof(double));{box_stmt_inline} }} else {{ memcpy(&d{v}, &recv_obj->slots[recv_row[{col_idx}]], sizeof(double));{box_stmt_heap} }}{int_sync}"
                    )
                    .unwrap();
                } else {
                    writeln!(
                        output,
                        "    v{v} = recv_row[{col_idx}] < KILN_C99_INLINE_SLOTS ? recv_obj->inline_slots[recv_row[{col_idx}]] : recv_obj->slots[recv_row[{col_idx}]];{int_sync}"
                    )
                    .unwrap();
                }
            }
            _ => unreachable!(),
        }
        return;
    }
    let poly_read = if ctx.plan.compact {
        format!(
            "else if (shape != NULL) {{ \
                kiln_val poly_v = ku_pic_poly_read(obj, &site); \
                if (poly_v != KILN_VAL_HOLE) {{ \
                    v{v} = poly_v; \
                    handled = 1; \
                }} \
            }}"
        )
    } else {
        format!(
            "else if (shape != NULL) {{ \
                for (int i = 1; i < (int)kiln_c99_site_count(&site) && i < 4; ++i) {{ \
                    if (shape == kiln_c99_slot_key(&site.entries[i].shape)) {{ \
                        uint32_t slot = site.entries[i].slot; \
                        if (slot < obj->slot_capacity) {{ v{v} = obj->slots[slot]; handled = 1; break; }} \
                        if (kiln_c99_pic_inherited(&site.entries[i], obj, &v{v})) {{ handled = 1; break; }} \
                    }} \
                }} \
            }}"
        )
    };
    writeln!(
                output,
                "    {{ int handled = 0; {{ static kiln_c99_site site = {{0}}; \
                {receiver_tier}{length_check}{size_check}\
                if (!handled && KILN_VAL_IS_OBJECT(v{o})) {{ \
                    const kiln_object *obj = (const kiln_object *)KILN_VAL_PTR(v{o}); \
                    const kiln_shape *shape = obj->shape; \
                    if (KILN_LIKELY(shape != NULL && shape == kiln_c99_slot_key(&site.entries[0].shape))) {{ \
                        uint32_t s0 = site.entries[0].slot; \
                        if (KILN_LIKELY(s0 < obj->slot_capacity)) {{ \
                            v{v} = obj->slots[s0]; \
                            handled = 1; \
                        }} else if (kiln_c99_pic_inherited(&site.entries[0], obj, &v{v})) {{ \
                            handled = 1; \
                        }} \
                    }} {poly_read} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_read_property_key(host, v{o}, {symbol}_key_{name}, depth, thrown, &site, &v{v}); \
                    if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_property_refusal(kiln_val_to_boundary(v{o})); \
                    {check} \
                }}{receiver_learn} }}\
                {typed_sync} }}",
                o = object.0,
                typed_sync = {
                    // Guessed a number because it feeds arithmetic: checked here, and the guess
                    // withdrawn by leaving for the generic clone, which reads it boxed as always.
                    let is_length = key_str == "length";
                    let skip_fast = (is_typed_entry && typed && this_site.is_some()) || is_length;
                    let restart_checked = if ctx.kind.restarts() && (typed || ctx.ints.contains(&vid)) {
                        let invariant =
                            crate::prelude::lookup_speculation_invariant("property_number");
                        let handled_cond = if skip_fast {
                            "!handled && "
                        } else {
                            ""
                        };
                        format!(
                            "\n    /* Speculation exit [{}]: {} */ if ({handled_cond}KILN_UNLIKELY(!KILN_VAL_IS_NUMBER(v{v}))) {{ {} }}",
                            invariant.name, invariant.language_rule, super::super::clones::RESTART
                        )
                    } else {
                        String::new()
                    };
                    let checked = if spec.exits && spec.fields.contains(&vid) {
                        let invariant =
                            crate::prelude::lookup_speculation_invariant("property_number");
                        let handled_cond = if skip_fast {
                            "!handled && "
                        } else {
                            ""
                        };
                        format!(
                            "\n    /* Speculation exit [{}]: {} */ if ({handled_cond}KILN_UNLIKELY(!KILN_VAL_IS_NUMBER(v{v}))) {{ {} goto kiln_generic_resume_{v}; }}",
                            invariant.name,
                            invariant.language_rule,
                            super::materialize_all_sunk(graph, symbol, ctx.defs, ctx.array_slots)
                        )
                    } else {
                        String::new()
                    };
                    let memcpy_load = if typed {
                        if skip_fast {
                            format!("if (!handled) memcpy(&d{v}, &v{v}, sizeof(d{v}));")
                        } else {
                            format!("memcpy(&d{v}, &v{v}, sizeof(d{v}));")
                        }
                    } else {
                        String::new()
                    };
                    if typed_int {
                        let int_load = if typed {
                            if is_length {
                                format!("if (!handled) i{v} = (int64_t)d{v};")
                            } else {
                                format!("i{v} = (int64_t)d{v};")
                            }
                        } else {
                            if is_length {
                                format!("if (!handled) {{ double tmp; memcpy(&tmp, &v{v}, sizeof(tmp)); i{v} = (int64_t)tmp; }}")
                            } else {
                                format!("double tmp; memcpy(&tmp, &v{v}, sizeof(tmp)); i{v} = (int64_t)tmp;")
                            }
                        };
                        // An array's length, proven an integer: exact below 2^53, so the cast is the value.
                        format!("{checked}{restart_checked}\n    {memcpy_load} {int_load}")
                    } else if typed {
                        format!("{checked}{restart_checked}\n    {memcpy_load}")
                    } else {
                        checked
                    }
                },
            )
            .unwrap();
}

/// `GraphOp::SetProperty { object, name, value, .. } | GraphOp::SetPropertyStrict { .. }`, moved
/// out of `emit_graph_op`'s match verbatim.
pub(super) fn emit_set_property(
    output: &mut String,
    vid: ValueId,
    op: &GraphOp,
    object: &ValueId,
    name: &u32,
    value: &ValueId,
    ctx: &OpContext,
) {
    let v = vid.0;
    let val = value.0;
    let graph = ctx.graph;
    let symbol = ctx.symbol;
    let diagnostic_site = ctx.diagnostic_site;
    let unwind = ctx.unwind;
    let typed = ctx.live_numbers.contains(&vid);
    let typed_int = ctx.live_ints.contains(&vid);
    let is_used = ctx.used.contains(&vid);
    let assign_v = if is_used {
        format!("v{v} = v{val}; ")
    } else {
        String::new()
    };
    let typed_sync = if is_used && typed {
        format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));")
    } else {
        String::new()
    };
    let (pc, label) = ctx
        .origins
        .get(&vid)
        .map(|o| (o.pc, o.label.as_str()))
        .unwrap_or((0, "Op"));
    if let Some(&root) = graph.scalar_alias_map.get(object)
        && let Some(scalar_obj) = graph.scalar_objects.get(&root)
    {
        let alloc = root.0;
        let val = value.0;
        if let Some(&field_idx) = scalar_obj.field_by_name_index.get(name) {
            writeln!(output, "    v{alloc}_f_{field_idx} = v{val};").unwrap();
            if ctx.live_numbers.contains(value) {
                writeln!(output, "    d{alloc}_f_{field_idx} = d{val};").unwrap();
            } else if ctx.live_ints.contains(value) {
                writeln!(output, "    d{alloc}_f_{field_idx} = (double)i{val};").unwrap();
            } else {
                writeln!(
                    output,
                    "    memcpy(&d{alloc}_f_{field_idx}, &v{alloc}_f_{field_idx}, sizeof(double));"
                )
                .unwrap();
            }
            if ctx.live_ints.contains(value) {
                writeln!(output, "    i{alloc}_f_{field_idx} = i{val};").unwrap();
            } else if ctx.live_numbers.contains(value) {
                writeln!(output, "    i{alloc}_f_{field_idx} = (int64_t)d{val};").unwrap();
            } else {
                writeln!(output, "    i{alloc}_f_{field_idx} = 0;").unwrap();
            }
        }
        if is_used {
            writeln!(output, "    v{v} = v{val};").unwrap();
            if typed {
                if ctx.live_numbers.contains(value) {
                    writeln!(output, "    d{v} = d{val};").unwrap();
                } else if ctx.live_ints.contains(value) {
                    writeln!(output, "    d{v} = (double)i{val};").unwrap();
                }
            }
            if typed_int {
                if ctx.live_ints.contains(value) {
                    writeln!(output, "    i{v} = i{val};").unwrap();
                } else if ctx.live_numbers.contains(value) {
                    writeln!(output, "    i{v} = (int64_t)d{val};").unwrap();
                }
            }
        }
        return;
    }
    let strict = matches!(op, GraphOp::SetPropertyStrict { strict: true, .. }) as u8;
    let check = status_check(diagnostic_site, pc, label, unwind);
    if ctx.plan.generic_only {
        if crate::compact_diagnostic_site(diagnostic_site) {
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; KU_WRITE_PROPERTY(v{o}, {symbol}_key_{name}, v{val}, {strict}, &site, {pc}, \"{label}\", {unwind}); {assign_v}{typed_sync} }}",
                o = object.0,
                val = value.0,
                assign_v = assign_v.trim_end(),
                typed_sync = typed_sync,
            )
            .unwrap();
        } else {
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; int status = kiln_c99_val_write_property_key(host, v{o}, {symbol}_key_{name}, v{val}, {strict}, depth, thrown, &site); {check} {assign_v}}}{typed_sync}",
                o = object.0,
                val = value.0,
                check = check,
                assign_v = assign_v,
                typed_sync = typed_sync,
            )
            .unwrap();
        }
        return;
    }
    let poly_write = if ctx.plan.compact {
        format!(
            "else if (shape != NULL) {{ \
                if (ku_pic_poly_write(obj, &site, v{val})) {{ \
                    {assign_v}\
                    handled = 1; \
                }} \
            }}"
        )
    } else {
        format!(
            "else if (shape != NULL) {{ \
                uint32_t slot = UINT32_MAX; \
                if (shape == kiln_c99_slot_key(&site.entries[1].shape) && site.entries[1].transition_to == NULL) slot = site.entries[1].slot; \
                else if (shape == kiln_c99_slot_key(&site.entries[2].shape) && site.entries[2].transition_to == NULL) slot = site.entries[2].slot; \
                else if (shape == kiln_c99_slot_key(&site.entries[3].shape) && site.entries[3].transition_to == NULL) slot = site.entries[3].slot; \
                if (slot != UINT32_MAX && KILN_LIKELY(slot < obj->slot_capacity && !obj->frozen)) {{ \
                    kiln_c99_prototype_touched(obj); \
                    obj->slots[slot] = v{val}; \
                    {assign_v}\
                    handled = 1; \
                }} \
            }}"
        )
    };
    writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; \
                int handled = 0; \
                if (KILN_VAL_IS_OBJECT(v{o})) {{ \
                    kiln_object *obj = (kiln_object *)KILN_VAL_PTR(v{o}); \
                    const kiln_shape *shape = obj->shape; \
                    if (KILN_LIKELY(shape != NULL && shape == kiln_c99_slot_key(&site.entries[0].shape) && site.entries[0].transition_to == NULL)) {{ \
                        uint32_t s0 = site.entries[0].slot; \
                        if (KILN_LIKELY(s0 < obj->slot_capacity && !obj->frozen)) {{ \
                            kiln_c99_prototype_touched(obj); obj->slots[s0] = v{val}; \
                            {assign_v}\
                            handled = 1; \
                        }} \
                    }} {poly_write} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_write_property_key(host, v{o}, {symbol}_key_{name}, v{val}, {strict}, depth, thrown, &site); \
                    {check} \
                    {assign_v}\
                }} }}\
                {typed_sync}",
                o = object.0,
                val = value.0,
                assign_v = assign_v,
                poly_write = poly_write,
                check = check,
                typed_sync = typed_sync,
            )
            .unwrap();
}

/// `GraphOp::GetComputed { object, key, .. }`, moved out of `emit_graph_op`'s match verbatim.
///
/// An array and a typed array are both buffer-tagged, and the payload's kind tells them apart
/// on every access. Until 2026-09-27 a site that had once seen an array skipped the question
/// from then on, so a typed view reaching it later was read as a `kiln_array` -- its buffer
/// pointer as the elements, its byte offset as the count -- and the element write and `length`
/// read beside it did the same (`conformance/native/typed-array-after-array-site.js`).
pub(super) fn emit_get_computed(
    output: &mut String,
    vid: ValueId,
    object: &ValueId,
    key: &ValueId,
    ctx: &OpContext,
) {
    let v = vid.0;
    let graph = ctx.graph;
    let program = ctx.program;
    let diagnostic_site = ctx.diagnostic_site;
    let ints = ctx.ints;
    let defs = ctx.defs;
    let unwind = ctx.unwind;
    let spec = ctx.spec;
    let typed = ctx.live_numbers.contains(&vid);
    let typed_int = ctx.live_ints.contains(&vid);
    let (pc, label) = ctx
        .origins
        .get(&vid)
        .map(|o| (o.pc, o.label.as_str()))
        .unwrap_or((0, "Op"));
    if let Some(&root) = graph.scalar_alias_map.get(object)
        && let Some(scalar_obj) = graph.scalar_objects.get(&root)
        && let Some(key_str) = resolve_const_string(*key, defs, program)
    {
        let alloc = root.0;
        if let Some(&field_idx) = scalar_obj.fields.get(&key_str) {
            writeln!(output, "    v{v} = v{alloc}_f_{field_idx};").unwrap();
            if typed {
                writeln!(output, "    d{v} = d{alloc}_f_{field_idx};").unwrap();
            }
            if typed_int {
                writeln!(output, "    i{v} = i{alloc}_f_{field_idx};").unwrap();
            }
        } else {
            writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap();
            if typed {
                writeln!(output, "    d{v} = 0.0;").unwrap();
            }
            if typed_int {
                writeln!(output, "    i{v} = 0;").unwrap();
            }
        }
        return;
    }
    let check = status_check(diagnostic_site, pc, label, unwind);
    if ctx.plan.generic_only {
        writeln!(
            output,
            "    {{ int status = kiln_c99_val_read_element(host, v{o}, v{k}, depth, thrown, &v{v}); if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_computed_refusal(kiln_val_to_boundary(v{o}), kiln_val_to_boundary(v{k})); {check} }}{typed_sync}",
            o = object.0,
            k = key.0,
            typed_sync = if typed {
                format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));")
            } else {
                String::new()
            }
        )
        .unwrap();
        return;
    }
    let key_is_int = ints.contains(key);
    let key_non_neg = ctx.ivs.get(key).map_or(false, |iv| iv.non_negative);
    let bce_proven = if let Some(iv) = ctx.ivs.get(key) {
        if iv.non_negative && iv.step > 0 {
            match ctx.defs.get(&iv.bound) {
                Some(GraphOp::GetProperty {
                    object: bound_obj,
                    name,
                    ..
                }) => {
                    let is_length = matches!(
                        program.constants.get(*name as usize),
                        Some(Constant::String(s)) if s == "length"
                    );
                    if is_length {
                        if bound_obj == object {
                            true
                        } else {
                            match (ctx.defs.get(bound_obj), ctx.defs.get(object)) {
                                (
                                    Some(GraphOp::LoadUpvalue(idx_a)),
                                    Some(GraphOp::LoadUpvalue(idx_b)),
                                ) => idx_a == idx_b,
                                (
                                    Some(GraphOp::LoadLocal(slot_a)),
                                    Some(GraphOp::LoadLocal(slot_b)),
                                ) => slot_a == slot_b,
                                _ => false,
                            }
                        }
                    } else {
                        false
                    }
                }
                _ => false,
            }
        } else {
            false
        }
    } else {
        false
    };
    // The object an array by the entry guard: no tag or kind to test, and an index
    // proven an integer is the element's index as it is.
    let array_tier = if spec.arrays.contains(object) && key_is_int {
        let cond = if bce_proven {
            format!(
                "kiln_spec_{o}_arr->elements[i{k}] != KILN_VAL_HOLE",
                o = object.0,
                k = key.0
            )
        } else if key_non_neg {
            format!(
                "(uint64_t)i{k} < kiln_spec_{o}_arr->count && kiln_spec_{o}_arr->elements[i{k}] != KILN_VAL_HOLE",
                o = object.0,
                k = key.0
            )
        } else {
            format!(
                "i{k} >= 0 && (uint64_t)i{k} < kiln_spec_{o}_arr->count && kiln_spec_{o}_arr->elements[i{k}] != KILN_VAL_HOLE",
                o = object.0,
                k = key.0
            )
        };
        format!(
            "if (KILN_LIKELY({cond})) {{ v{v} = kiln_spec_{o}_arr->elements[i{k}]; handled = 1; }} ",
            o = object.0,
            k = key.0,
            v = v,
            cond = cond
        )
    } else if spec.arrays.contains(object) {
        format!(
            "if (KILN_VAL_IS_NUMBER(v{k})) {{ const kiln_array *arr = kiln_spec_{o}_arr; double idx; memcpy(&idx, &v{k}, sizeof(idx)); size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < arr->count && arr->elements[slot] != KILN_VAL_HOLE)) {{ v{v} = arr->elements[slot]; handled = 1; }} }} ",
            o = object.0,
            k = key.0,
            v = v
        )
    } else {
        String::new()
    };
    let (vk_decl, vk) = if key_is_int {
        (
            format!(
                "kiln_val vk = !handled ? kiln_c99_val_from_double((double)i{}) : 0; ",
                key.0
            ),
            "vk".to_string(),
        )
    } else {
        (String::new(), format!("v{}", key.0))
    };
    let (array_body, typed_body, string_body) = if key_is_int {
        let arr_bound = if bce_proven {
            String::new()
        } else if key_non_neg {
            format!("(uint64_t)i{k} < arr->count && ", k = key.0)
        } else {
            format!("i{k} >= 0 && (uint64_t)i{k} < arr->count && ", k = key.0)
        };
        let typed_bound = if key_non_neg {
            format!(
                "(uint64_t)i{k} < view->length && !view->buffer->detached && view->kind <= KILN_TYPED_FLOAT64",
                k = key.0
            )
        } else {
            format!(
                "i{k} >= 0 && (uint64_t)i{k} < view->length && !view->buffer->detached && view->kind <= KILN_TYPED_FLOAT64",
                k = key.0
            )
        };
        let str_bound = if key_non_neg {
            format!("(uint64_t)i{k} < str->unit_len", k = key.0)
        } else {
            format!("i{k} >= 0 && (uint64_t)i{k} < str->unit_len", k = key.0)
        };
        (
            format!(
                "const kiln_array *arr = (const kiln_array *)ptr; if (KILN_LIKELY({arr_bound}arr->elements[i{k}] != KILN_VAL_HOLE)) {{ v{v} = arr->elements[i{k}]; handled = 1; }}",
                k = key.0,
                v = v,
                arr_bound = arr_bound
            ),
            format!(
                "kiln_typed_array *view = (kiln_typed_array *)ptr; if (KILN_LIKELY({typed_bound})) {{ v{v} = kiln_c99_val_from_double(kiln_typed_load(view, (size_t)i{k})); handled = 1; }}",
                k = key.0,
                v = v
            ),
            format!(
                "const kiln_string *str = (const kiln_string *)KILN_VAL_PTR(v{o}); if (KILN_LIKELY(str != NULL && {str_bound})) {{ v{v} = kiln_c99_val_from_unit(kiln_string_unit_at(str, (size_t)i{k})); handled = 1; }}",
                o = object.0,
                k = key.0,
                v = v
            ),
        )
    } else {
        (
            format!(
                "double idx; memcpy(&idx, &{vk}, sizeof(idx)); size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; const kiln_array *arr = (const kiln_array *)ptr; if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < arr->count && arr->elements[slot] != KILN_VAL_HOLE)) {{ v{v} = arr->elements[slot]; handled = 1; }}",
                vk = vk,
                v = v
            ),
            format!(
                "double idx; memcpy(&idx, &{vk}, sizeof(idx)); size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; kiln_typed_array *view = (kiln_typed_array *)ptr; if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < view->length && !view->buffer->detached && view->kind <= KILN_TYPED_FLOAT64)) {{ v{v} = kiln_c99_val_from_double(kiln_typed_load(view, slot)); handled = 1; }}",
                vk = vk,
                v = v
            ),
            format!(
                "double idx; memcpy(&idx, &{vk}, sizeof(idx)); size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; const kiln_string *str = (const kiln_string *)KILN_VAL_PTR(v{o}); if (KILN_LIKELY(str != NULL && idx >= 0.0 && idx == (double)slot && slot < str->unit_len)) {{ v{v} = kiln_c99_val_from_unit(kiln_string_unit_at(str, slot)); handled = 1; }}",
                vk = vk,
                o = object.0,
                v = v
            ),
        )
    };
    let number_guard = if key_is_int {
        String::new()
    } else {
        format!(" && KILN_VAL_IS_NUMBER({vk})")
    };
    writeln!(
                output,
                "    {{ int handled = 0; {array_tier}{vk_decl}\
                if (!handled && KILN_LIKELY((v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX{number_guard})) {{ \
                    void *ptr = KILN_VAL_PTR(v{o}); \
                    uint32_t kind = kiln_c99_gc_payload_kind(ptr); \
                    if (kind == KILN_GC_ARRAY) {{ \
                        {array_body} \
                    }} else if (kind == KILN_GC_TYPED_ARRAY) {{ \
                        {typed_body} \
                    }} \
                }} else if (!handled && KILN_VAL_IS_STRING(v{o}){number_guard}) {{ \
                    {string_body} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_read_element(host, v{o}, {vk}, depth, thrown, &v{v}); \
                    if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_computed_refusal(kiln_val_to_boundary(v{o}), kiln_val_to_boundary({vk})); \
                    {check} \
                }} }}\
                {typed_sync}",
                o = object.0,
                number_guard = number_guard,
                array_body = array_body,
                typed_body = typed_body,
                string_body = string_body,
                vk = vk,
                typed_sync = if typed { format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));") } else { String::new() }
            )
            .unwrap();
}

/// `GraphOp::SetComputed { object, key, value, .. } | GraphOp::SetComputedStrict { .. }`, moved
/// out of `emit_graph_op`'s match verbatim.
pub(super) fn emit_set_computed(
    output: &mut String,
    vid: ValueId,
    op: &GraphOp,
    object: &ValueId,
    key: &ValueId,
    value: &ValueId,
    ctx: &OpContext,
) {
    let v = vid.0;
    let val = value.0;
    let graph = ctx.graph;
    let program = ctx.program;
    let diagnostic_site = ctx.diagnostic_site;
    let ints = ctx.ints;
    let defs = ctx.defs;
    let unwind = ctx.unwind;
    let typed = ctx.live_numbers.contains(&vid);
    let typed_int = ctx.live_ints.contains(&vid);
    let is_used = ctx.used.contains(&vid);
    let assign_v = if is_used {
        format!("v{v} = v{val}; ")
    } else {
        String::new()
    };
    let typed_sync = if is_used && typed {
        format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));")
    } else {
        String::new()
    };
    let (pc, label) = ctx
        .origins
        .get(&vid)
        .map(|o| (o.pc, o.label.as_str()))
        .unwrap_or((0, "Op"));
    if let Some(&root) = graph.scalar_alias_map.get(object)
        && let Some(scalar_obj) = graph.scalar_objects.get(&root)
        && let Some(key_str) = resolve_const_string(*key, defs, program)
    {
        let alloc = root.0;
        let val = value.0;
        if let Some(&field_idx) = scalar_obj.fields.get(&key_str) {
            writeln!(output, "    v{alloc}_f_{field_idx} = v{val};").unwrap();
            if ctx.live_numbers.contains(value) {
                writeln!(output, "    d{alloc}_f_{field_idx} = d{val};").unwrap();
            } else if ctx.live_ints.contains(value) {
                writeln!(output, "    d{alloc}_f_{field_idx} = (double)i{val};").unwrap();
            } else {
                writeln!(
                    output,
                    "    memcpy(&d{alloc}_f_{field_idx}, &v{alloc}_f_{field_idx}, sizeof(double));"
                )
                .unwrap();
            }
            if ctx.live_ints.contains(value) {
                writeln!(output, "    i{alloc}_f_{field_idx} = i{val};").unwrap();
            } else if ctx.live_numbers.contains(value) {
                writeln!(output, "    i{alloc}_f_{field_idx} = (int64_t)d{val};").unwrap();
            } else {
                writeln!(output, "    i{alloc}_f_{field_idx} = 0;").unwrap();
            }
        }
        if is_used {
            writeln!(output, "    v{v} = v{val};").unwrap();
            if typed {
                if ctx.live_numbers.contains(value) {
                    writeln!(output, "    d{v} = d{val};").unwrap();
                } else if ctx.live_ints.contains(value) {
                    writeln!(output, "    d{v} = (double)i{val};").unwrap();
                }
            }
            if typed_int {
                if ctx.live_ints.contains(value) {
                    writeln!(output, "    i{v} = i{val};").unwrap();
                } else if ctx.live_numbers.contains(value) {
                    writeln!(output, "    i{v} = (int64_t)d{val};").unwrap();
                }
            }
        }
        return;
    }
    let strict = matches!(op, GraphOp::SetComputedStrict { strict: true, .. }) as u8;
    let check = status_check(diagnostic_site, pc, label, unwind);
    if ctx.plan.generic_only {
        writeln!(
            output,
            "    {{ int status = kiln_c99_val_write_element(host, v{o}, v{k}, v{val}, {strict}, depth, thrown); {check} {assign_v}}}{typed_sync}",
            o = object.0,
            k = key.0,
            val = value.0,
            check = check,
            assign_v = assign_v,
            typed_sync = typed_sync,
        )
        .unwrap();
        return;
    }
    let key_is_int = ints.contains(key);
    let key_non_neg = ctx.ivs.get(key).map_or(false, |iv| iv.non_negative);
    let (array_body, typed_body) = if key_is_int {
        let arr_bound = if key_non_neg {
            format!("(uint64_t)i{k} < arr->count", k = key.0)
        } else {
            format!("i{k} >= 0 && (uint64_t)i{k} < arr->count", k = key.0)
        };
        let typed_bound = if key_non_neg {
            format!(
                "(uint64_t)i{k} < view->length && !view->buffer->detached && view->kind <= KILN_TYPED_FLOAT64",
                k = key.0
            )
        } else {
            format!(
                "i{k} >= 0 && (uint64_t)i{k} < view->length && !view->buffer->detached && view->kind <= KILN_TYPED_FLOAT64",
                k = key.0
            )
        };
        let val_d_init = if ctx.live_numbers.contains(value) {
            format!("double val_d = d{val};", val = value.0)
        } else if ctx.live_ints.contains(value) {
            format!("double val_d = (double)i{val};", val = value.0)
        } else {
            format!(
                "double val_d; if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{val}))) memcpy(&val_d, &v{val}, sizeof(val_d)); else val_d = kiln_c99_as_number(kiln_val_to_boundary(v{val}));",
                val = value.0
            )
        };
        (
            format!(
                "kiln_array *arr = (kiln_array *)ptr; if (KILN_LIKELY({arr_bound} && !arr->frozen)) {{ if (arr->is_numeric && !KILN_VAL_IS_NUMBER(v{val})) arr->is_numeric = 0; arr->elements[i{k}] = v{val}; {assign_v}handled = 1; }}",
                k = key.0,
                val = value.0,
                assign_v = assign_v,
            ),
            format!(
                "kiln_typed_array *view = (kiln_typed_array *)ptr; if (KILN_LIKELY({typed_bound})) {{ {val_d_init} kiln_c99_typed_set(view, (size_t)i{k}, val_d); {assign_v}handled = 1; }}",
                k = key.0,
                val_d_init = val_d_init,
                assign_v = assign_v,
            ),
        )
    } else {
        let val_d_init = if ctx.live_numbers.contains(value) {
            format!("double val_d = d{val};", val = value.0)
        } else if ctx.live_ints.contains(value) {
            format!("double val_d = (double)i{val};", val = value.0)
        } else {
            format!(
                "double val_d; if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{val}))) memcpy(&val_d, &v{val}, sizeof(val_d)); else val_d = kiln_c99_as_number(kiln_val_to_boundary(v{val}));",
                val = value.0
            )
        };
        (
            format!(
                "double idx; memcpy(&idx, &v{k}, sizeof(idx)); size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; kiln_array *arr = (kiln_array *)ptr; if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < arr->count && !arr->frozen)) {{ if (arr->is_numeric && !KILN_VAL_IS_NUMBER(v{val})) arr->is_numeric = 0; arr->elements[slot] = v{val}; {assign_v}handled = 1; }}",
                k = key.0,
                val = value.0,
                assign_v = assign_v,
            ),
            format!(
                "double idx; memcpy(&idx, &v{k}, sizeof(idx)); size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; kiln_typed_array *view = (kiln_typed_array *)ptr; if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < view->length && !view->buffer->detached)) {{ {val_d_init} kiln_c99_typed_set(view, slot, val_d); {assign_v}handled = 1; }}",
                k = key.0,
                val_d_init = val_d_init,
                assign_v = assign_v,
            ),
        )
    };
    let (vk_decl, vk) = if key_is_int {
        (
            format!(
                "kiln_val vk = !handled ? kiln_c99_val_from_double((double)i{}) : 0; ",
                key.0
            ),
            "vk".to_string(),
        )
    } else {
        (String::new(), format!("v{}", key.0))
    };
    let number_guard = if key_is_int {
        String::new()
    } else {
        format!(" && KILN_VAL_IS_NUMBER(v{k})", k = key.0)
    };
    writeln!(
                output,
                "    {{ int handled = 0; {vk_decl}\
                if (KILN_LIKELY((v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX{number_guard})) {{ \
                    void *ptr = KILN_VAL_PTR(v{o}); \
                    uint32_t kind = kiln_c99_gc_payload_kind(ptr); \
                    if (kind == KILN_GC_ARRAY) {{ \
                        {array_body} \
                    }} else if (kind == KILN_GC_TYPED_ARRAY) {{ \
                        {typed_body} \
                    }} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_write_element(host, v{o}, {vk}, v{val}, {strict}, depth, thrown); \
                    {check} \
                    {assign_v}\
                }} }}\
                {typed_sync}",
                o = object.0,
                val = value.0,
                vk_decl = vk_decl,
                vk = vk,
                number_guard = number_guard,
                array_body = array_body,
                typed_body = typed_body,
                check = check,
                assign_v = assign_v,
                typed_sync = typed_sync,
            )
            .unwrap();
}
