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
    let typed = ctx.numbers.contains(&vid);
    let typed_int = ctx.ints.contains(&vid);
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
        let typed_sync = if typed_int {
            format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v})); i{v} = (int64_t)d{v};")
        } else if typed {
            format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));")
        } else {
            String::new()
        };
        if crate::compact_diagnostic_site(diagnostic_site) {
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; KU_READ_PROPERTY(v{v}, v{o}, {symbol}_key_{name}, {key_len}, &site, {pc}, \"{label}\", {unwind}); /* goto kiln_refusal; */ }}{typed_sync}",
                o = object.0,
            )
            .unwrap();
        } else {
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; int status = kiln_c99_val_read_property(host, v{o}, {symbol}_key_{name}, {key_len}, depth, thrown, &site, &v{v}); if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_property_refusal(kiln_val_to_boundary(v{o})); {check} }}{typed_sync}",
                o = object.0,
            )
            .unwrap();
        }
        return;
    }
    let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
    let length_check = if key_str == "length" && spec.arrays.contains(object) {
        let box_stmt = if need_box {
            format!(" v{v} = kiln_c99_val_from_double((double)arr->count);")
        } else {
            String::new()
        };
        let int_stmt = if typed_int {
            format!(" i{v} = (int64_t)arr->count;")
        } else {
            String::new()
        };
        let double_stmt = if typed {
            format!(" d{v} = (double)arr->count;")
        } else {
            String::new()
        };
        format!(
            "{{ const kiln_array *arr = (const kiln_array *)KILN_VAL_PTR(v{o});{int_stmt}{double_stmt}{box_stmt} handled = 1; }} ",
            o = object.0
        )
    } else if key_str == "length" {
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
                         if (KILN_LIKELY(site.has_cached_method == 1 || kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY)) {{ \
                             site.has_cached_method = 1; \
                             {int_stmt}{double_stmt}{box_stmt} \
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
                let box_stmt = if need_box {
                    format!(" v{v} = recv_obj->slots[recv_row[{i}]];")
                } else {
                    String::new()
                };
                (
                    format!(
                        "if (recv_row != NULL) {{ memcpy(&d{v}, &recv_obj->slots[recv_row[{i}]], sizeof(double));{box_stmt} handled = 1; }} "
                    ),
                    String::new(),
                )
            } else {
                (
                    format!(
                        "if (recv_row != NULL) {{ v{v} = recv_obj->slots[recv_row[{i}]]; handled = 1; }} "
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
                let box_stmt = if need_box {
                    format!(" v{v} = recv_obj->slots[recv_row[{col_idx}]];")
                } else {
                    String::new()
                };
                if typed {
                    writeln!(
                        output,
                        "    memcpy(&d{v}, &recv_obj->slots[recv_row[{col_idx}]], sizeof(double));{box_stmt}{int_sync}"
                    )
                    .unwrap();
                } else {
                    writeln!(
                        output,
                        "    v{v} = recv_obj->slots[recv_row[{col_idx}]];{int_sync}"
                    )
                    .unwrap();
                }
            }
            _ => unreachable!(),
        }
        return;
    }
    writeln!(
                output,
                "    {{ int handled = 0; {{ static kiln_c99_site site = {{0}}; \
                {receiver_tier}{length_check}{size_check}\
                if (!handled && KILN_VAL_IS_OBJECT(v{o})) {{ \
                    const kiln_object *obj = (const kiln_object *)KILN_VAL_PTR(v{o}); \
                    const kiln_shape *shape = obj->shape; \
                    uint32_t slot = UINT32_MAX; \
                    if (KILN_LIKELY(shape != NULL)) {{ \
                        if (shape == site.entries[0].shape) slot = site.entries[0].slot; \
                        else if (shape == site.entries[1].shape) slot = site.entries[1].slot; \
                        else if (shape == site.entries[2].shape) slot = site.entries[2].slot; \
                        else if (shape == site.entries[3].shape) slot = site.entries[3].slot; \
                    }} \
                    if (slot != UINT32_MAX && KILN_LIKELY(slot < obj->slot_capacity)) {{ \
                        v{v} = obj->slots[slot]; \
                        handled = 1; \
                    }} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_read_property(host, v{o}, {symbol}_key_{name}, {key_len}, depth, thrown, &site, &v{v}); \
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
                    let memcpy_load = if skip_fast {
                        format!("if (!handled) memcpy(&d{v}, &v{v}, sizeof(d{v}));")
                    } else {
                        format!("memcpy(&d{v}, &v{v}, sizeof(d{v}));")
                    };
                    if ctx.ints.contains(&vid) {
                        let int_load = if is_length {
                            format!("if (!handled) i{v} = (int64_t)d{v};")
                        } else {
                            format!("i{v} = (int64_t)d{v};")
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
    let graph = ctx.graph;
    let program = ctx.program;
    let symbol = ctx.symbol;
    let diagnostic_site = ctx.diagnostic_site;
    let numbers = ctx.numbers;
    let ints = ctx.ints;
    let unwind = ctx.unwind;
    let typed = numbers.contains(&vid);
    let typed_int = ints.contains(&vid);
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
            if numbers.contains(value) {
                writeln!(output, "    d{alloc}_f_{field_idx} = d{val};").unwrap();
            } else {
                writeln!(
                    output,
                    "    memcpy(&d{alloc}_f_{field_idx}, &v{alloc}_f_{field_idx}, sizeof(double));"
                )
                .unwrap();
            }
            if ints.contains(value) {
                writeln!(output, "    i{alloc}_f_{field_idx} = i{val};").unwrap();
            } else {
                writeln!(output, "    i{alloc}_f_{field_idx} = 0;").unwrap();
            }
        }
        writeln!(output, "    v{v} = v{val};").unwrap();
        if typed {
            writeln!(output, "    d{v} = d{val};").unwrap();
        }
        if typed_int {
            writeln!(output, "    i{v} = i{val};").unwrap();
        }
        return;
    }
    let key_str = match program.constants.get(*name as usize) {
        Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
        _ => String::new(),
    };
    let key_len = key_str.len();
    let strict = matches!(op, GraphOp::SetPropertyStrict { strict: true, .. }) as u8;
    let check = status_check(diagnostic_site, pc, label, unwind);
    if ctx.plan.generic_only {
        writeln!(
            output,
            "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; int status = kiln_c99_val_write_property(host, v{o}, {symbol}_key_{name}, {key_len}, v{val}, {strict}, depth, thrown, &site); {check} v{v} = v{val}; }}{typed_sync}",
            o = object.0,
            val = value.0,
            typed_sync = if typed {
                format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));")
            } else {
                String::new()
            }
        )
        .unwrap();
        return;
    }
    writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; \
                int handled = 0; \
                if (KILN_VAL_IS_OBJECT(v{o})) {{ \
                    kiln_object *obj = (kiln_object *)KILN_VAL_PTR(v{o}); \
                    const kiln_shape *shape = obj->shape; \
                    uint32_t slot = UINT32_MAX; \
                    if (KILN_LIKELY(shape != NULL)) {{ \
                        if (shape == site.entries[0].shape && site.entries[0].transition_to == NULL) slot = site.entries[0].slot; \
                        else if (shape == site.entries[1].shape && site.entries[1].transition_to == NULL) slot = site.entries[1].slot; \
                        else if (shape == site.entries[2].shape && site.entries[2].transition_to == NULL) slot = site.entries[2].slot; \
                        else if (shape == site.entries[3].shape && site.entries[3].transition_to == NULL) slot = site.entries[3].slot; \
                    }} \
                    if (slot != UINT32_MAX && KILN_LIKELY(slot < obj->slot_capacity && !obj->frozen)) {{ \
                        kiln_c99_prototype_touched(obj); obj->slots[slot] = v{val}; \
                        v{v} = v{val}; \
                        handled = 1; \
                    }} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_write_property(host, v{o}, {symbol}_key_{name}, {key_len}, v{val}, {strict}, depth, thrown, &site); \
                    {check} \
                    v{v} = v{val}; \
                }} }}\
                {typed_sync}",
                o = object.0,
                val = value.0,
                typed_sync = if typed { format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));") } else { String::new() }
            )
            .unwrap();
}

/// `GraphOp::GetComputed { object, key, .. }`, moved out of `emit_graph_op`'s match verbatim.
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
    let typed = ctx.numbers.contains(&vid);
    let typed_int = ints.contains(&vid);
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
    // The object an array by the entry guard: no tag or kind to test, and an index
    // proven an integer is the element's index as it is.
    let array_tier = if spec.arrays.contains(object) && ints.contains(key) {
        format!(
            "{{ const kiln_array *arr = (const kiln_array *)KILN_VAL_PTR(v{o}); if (KILN_LIKELY(i{k} >= 0 && (uint64_t)i{k} < arr->count && arr->elements[i{k}] != KILN_VAL_HOLE)) {{ v{v} = arr->elements[i{k}]; handled = 1; }} }} ",
            o = object.0,
            k = key.0
        )
    } else if spec.arrays.contains(object) {
        format!(
            "if (KILN_VAL_IS_NUMBER(v{k})) {{ const kiln_array *arr = (const kiln_array *)KILN_VAL_PTR(v{o}); double idx; memcpy(&idx, &v{k}, sizeof(idx)); size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < arr->count && arr->elements[slot] != KILN_VAL_HOLE)) {{ v{v} = arr->elements[slot]; handled = 1; }} }} ",
            o = object.0,
            k = key.0
        )
    } else {
        String::new()
    };
    let (vk_decl, vk) = if spec.arrays.contains(object) && ints.contains(key) {
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
    writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; \
                int handled = 0; {array_tier}{vk_decl}\
                if (!handled && KILN_LIKELY((v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_NUMBER({vk}))) {{ \
                    void *ptr = KILN_VAL_PTR(v{o}); \
                    if (site.has_cached_method == 1 || kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                        site.has_cached_method = 1; \
                        double idx; \
                        memcpy(&idx, &{vk}, sizeof(idx)); \
                        size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; \
                        const kiln_array *arr = (const kiln_array *)ptr; \
                        if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < arr->count && arr->elements[slot] != KILN_VAL_HOLE)) {{ \
                            v{v} = arr->elements[slot]; \
                            handled = 1; \
                        }} \
                    }} \
                }} else if (!handled && KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_NUMBER({vk})) {{ \
                    double idx; \
                    memcpy(&idx, &{vk}, sizeof(idx)); \
                    size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; \
                    const kiln_string *str = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                    if (KILN_LIKELY(str != NULL && idx >= 0.0 && idx == (double)slot && slot < str->unit_len)) {{ \
                        v{v} = kiln_c99_val_from_unit(kiln_string_unit_at(str, slot)); \
                        handled = 1; \
                    }} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_read_element(host, v{o}, {vk}, depth, thrown, &v{v}); \
                    if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_computed_refusal(kiln_val_to_boundary(v{o}), kiln_val_to_boundary({vk})); \
                    {check} \
                }} }}\
                {typed_sync}",
                o = object.0,
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
    let graph = ctx.graph;
    let program = ctx.program;
    let diagnostic_site = ctx.diagnostic_site;
    let numbers = ctx.numbers;
    let ints = ctx.ints;
    let defs = ctx.defs;
    let unwind = ctx.unwind;
    let typed = numbers.contains(&vid);
    let typed_int = ints.contains(&vid);
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
            if numbers.contains(value) {
                writeln!(output, "    d{alloc}_f_{field_idx} = d{val};").unwrap();
            } else {
                writeln!(
                    output,
                    "    memcpy(&d{alloc}_f_{field_idx}, &v{alloc}_f_{field_idx}, sizeof(double));"
                )
                .unwrap();
            }
            if ints.contains(value) {
                writeln!(output, "    i{alloc}_f_{field_idx} = i{val};").unwrap();
            } else {
                writeln!(output, "    i{alloc}_f_{field_idx} = 0;").unwrap();
            }
        }
        writeln!(output, "    v{v} = v{val};").unwrap();
        if typed {
            writeln!(output, "    d{v} = d{val};").unwrap();
        }
        if typed_int {
            writeln!(output, "    i{v} = i{val};").unwrap();
        }
        return;
    }
    let strict = matches!(op, GraphOp::SetComputedStrict { strict: true, .. }) as u8;
    let check = status_check(diagnostic_site, pc, label, unwind);
    if ctx.plan.generic_only {
        writeln!(
            output,
            "    {{ int status = kiln_c99_val_write_element(host, v{o}, v{k}, v{val}, {strict}, depth, thrown); {check} v{v} = v{val}; }}{typed_sync}",
            o = object.0,
            k = key.0,
            val = value.0,
            typed_sync = if typed {
                format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));")
            } else {
                String::new()
            }
        )
        .unwrap();
        return;
    }
    writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; \
                int handled = 0; \
                if (KILN_LIKELY((v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_NUMBER(v{k}))) {{ \
                    void *ptr = KILN_VAL_PTR(v{o}); \
                    if (site.has_cached_method == 1 || kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                        site.has_cached_method = 1; \
                        double idx; \
                        memcpy(&idx, &v{k}, sizeof(idx)); \
                        size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; \
                        kiln_array *arr = (kiln_array *)ptr; \
                        if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < arr->count && !arr->frozen)) {{ \
                            if (arr->is_numeric && !KILN_VAL_IS_NUMBER(v{val})) arr->is_numeric = 0; \
                            arr->elements[slot] = v{val}; \
                            v{v} = v{val}; \
                            handled = 1; \
                        }} \
                    }} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_write_element(host, v{o}, v{k}, v{val}, {strict}, depth, thrown); \
                    {check} \
                    v{v} = v{val}; \
                }} }}\
                {typed_sync}",
                o = object.0,
                k = key.0,
                val = value.0,
                typed_sync = if typed { format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));") } else { String::new() }
            )
            .unwrap();
}
