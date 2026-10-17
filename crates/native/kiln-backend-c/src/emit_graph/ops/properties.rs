use super::OpContext;
use super::emit_struct_copy_fields;
use super::status_check;
use crate::emit_graph::EntryKind;
use crate::graph::{GraphOp, ValueId, resolve_const_string};
use kiln_ir::Constant;
use std::fmt::Write;

fn prepared_method_property(vid: ValueId, ctx: &OpContext) -> bool {
    ctx.prepared_method_properties.contains(&vid)
}

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
    if ctx.plan.semantics == crate::NativeSemantics::Native {
        if let Some(&(_obj_lid, ref arr_expr)) =
            ctx.native_records.record_array_value_exprs.get(object)
        {
            let prop_name = match program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
                _ => String::new(),
            };
            if prop_name == "length" {
                let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
                if typed {
                    writeln!(output, "    d{v} = (double){arr_expr}.count;").unwrap();
                }
                if typed_int {
                    writeln!(output, "    i{v} = (int64_t){arr_expr}.count;").unwrap();
                }
                if need_box {
                    writeln!(
                        output,
                        "    v{v} = kiln_c99_val_from_double((double){arr_expr}.count);"
                    )
                    .unwrap();
                }
                return;
            }
        }
        if ctx.native_records.number_array_values.contains(object) {
            let prop_name = match program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
                _ => String::new(),
            };
            if prop_name == "length" {
                let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
                writeln!(
                    output,
                    "    {{ const kiln_array *arr = (const kiln_array *)KILN_VAL_PTR(v{});",
                    object.0
                )
                .unwrap();
                if typed {
                    writeln!(output, "    d{v} = (double)arr->count;").unwrap();
                }
                if typed_int {
                    writeln!(output, "    i{v} = (int64_t)arr->count;").unwrap();
                }
                if need_box {
                    writeln!(
                        output,
                        "    v{v} = kiln_c99_val_from_double((double)arr->count);"
                    )
                    .unwrap();
                }
                writeln!(output, "    }}").unwrap();
                return;
            }
        }
        if let Some(&(obj_lid, ref obj_expr)) = ctx.native_records.value_exprs.get(object) {
            let prop_name = match program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
                _ => String::new(),
            };
            let all_layouts = crate::collect_program_record_layouts(program);
            if let Some(layout) = all_layouts.get(obj_lid.0 as usize) {
                if let Some(field) = layout.fields.iter().find(|f| f.name == prop_name) {
                    let field_expr = format!("{obj_expr}.{prop_name}");
                    let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
                    match field.ty {
                        kiln_ir::NativeFieldType::Number => {
                            if typed {
                                writeln!(output, "    d{v} = {field_expr};").unwrap();
                            }
                            if typed_int {
                                writeln!(output, "    i{v} = (int64_t){field_expr};").unwrap();
                            }
                            if need_box {
                                writeln!(
                                    output,
                                    "    v{v} = kiln_c99_val_from_double({field_expr});"
                                )
                                .unwrap();
                            }
                        }
                        kiln_ir::NativeFieldType::Int32 => {
                            if typed_int {
                                writeln!(output, "    i{v} = (int64_t){field_expr};").unwrap();
                            }
                            if typed {
                                writeln!(output, "    d{v} = (double){field_expr};").unwrap();
                            }
                            if need_box {
                                writeln!(
                                    output,
                                    "    v{v} = kiln_c99_val_from_double((double){field_expr});"
                                )
                                .unwrap();
                            }
                        }
                        kiln_ir::NativeFieldType::Boolean => {
                            if ctx.booleans.contains(&vid) {
                                writeln!(output, "    b{v} = {field_expr};").unwrap();
                            }
                            if need_box {
                                writeln!(
                                    output,
                                    "    v{v} = ({field_expr}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
                                )
                                .unwrap();
                            }
                        }
                        kiln_ir::NativeFieldType::String => {
                            if need_box {
                                writeln!(output, "    v{v} = {field_expr};").unwrap();
                            }
                        }
                        kiln_ir::NativeFieldType::Record(sub_lid) => {
                            if ctx.native_records.boxed_record_values.contains(&vid) {
                                crate::emit_graph::emit_box_native_record(
                                    output,
                                    ctx.symbol,
                                    v,
                                    sub_lid,
                                    &field_expr,
                                    &all_layouts,
                                );
                            } else if need_box {
                                writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap();
                            }
                        }
                        kiln_ir::NativeFieldType::NumberArray => {
                            writeln!(output, "    v{v} = {field_expr};").unwrap();
                        }
                        kiln_ir::NativeFieldType::RecordArray(_) => {
                            if need_box
                                && !ctx
                                    .native_records
                                    .record_array_value_exprs
                                    .contains_key(&vid)
                            {
                                writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap();
                            }
                        }
                    }
                    return;
                }
            }
        }
    }
    if prepared_method_property(vid, ctx) {
        let key_str = match program.constants.get(*name as usize) {
            Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
            _ => String::new(),
        };
        let operation = crate::string::c_string(&format!("GetProperty({key_str})"));
        let operation = &operation[1..operation.len() - 1];
        let check = status_check(diagnostic_site, pc, operation, unwind);
        let source = crate::diagnostic_source(diagnostic_site);
        // A built-in namespace's method (`Math.sqrt`) read ahead of its arguments: once this
        // site has remembered the table's method for its own spelling, the read is a few
        // compares inline instead of a call into the runtime that re-derives the key, re-matches
        // the spelling and re-proves the member (`kiln_c99_cached_builtin_member_ok`: one compare
        // of the stores summary while no realm has modified a built-in, the realm's own record
        // otherwise). The site is write-once (`kiln_c99_site_cache_method`) and belongs to this
        // one constant key, so the spelling compare is the runtime's own hit test; any miss,
        // including a modified built-in, takes the runtime call below unchanged.
        let inline_hit = if key_str.is_ascii()
            && !key_str.is_empty()
            && key_str.len() < 32
            && !key_str.starts_with("@@")
        {
            let literal = crate::string::c_string(&key_str);
            let len = key_str.len();
            format!(
                "if (KILN_LIKELY(kiln_c99_site_has_method(&site) && site.cached_tag == KILN_VALUE_BUILTIN && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == site.cached_builtin && site.cached_member_len == {len} && memcmp(site.cached_member, {literal}, {len}) == 0 && kiln_c99_cached_builtin_member_ok(site.cached_builtin, site.cached_member, {len}))) {{ v{v} = KILN_VAL_METHOD(site.cached_method); }} else ",
                o = object.0,
            )
        } else {
            String::new()
        };
        let inv = crate::prelude::lookup_speculation_invariant("method_shape_cache");
        writeln!(
            output,
            "    {{ static kiln_c99_site site = {{0}}; {inline_hit}{{ kiln_val cached_fn = 0; \
                /* Inline method-shape cache [{inv_name}]: {inv_rule} */ \
                if (KILN_VAL_IS_OBJECT(v{o}) && kiln_c99_site_count(&site) > 0) {{ \
                    const kiln_object *obj = (const kiln_object *)KILN_VAL_PTR(v{o}); \
                    if (KILN_LIKELY(obj != NULL && kiln_c99_gc_payload_kind(obj) == KILN_GC_OBJECT && obj->shape != NULL)) cached_fn = kiln_c99_site_cached_callee(&site, obj); \
                }} \
                if (KILN_LIKELY(cached_fn != 0 && (KILN_VAL_IS_CLOSURE(cached_fn) || KILN_VAL_IS_METHOD(cached_fn)))) {{ v{v} = cached_fn; }} else {{ \
                int status = kiln_c99_val_prepare_method_key(host, v{o}, {symbol}_key_{name}, depth, thrown, &site, {source}, (size_t){pc}, \"{operation}\", &v{v}); if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_property_refusal(kiln_val_to_boundary(v{o})); {check} }} }} }}",
            inv_name = inv.name,
            inv_rule = inv.language_rule,
            o = object.0,
        )
        .unwrap();
        return;
    }
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
                             if (kiln_c99_collection_intrinsic_method_allowed(col, \"size\", sizeof(\"size\") - 1)) {{ \
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
    // A receiver row is valid only while `this` still has the shape it had at entry.
    // Calls can delete or redefine properties, which materializes the object as a
    // dictionary while leaving its old slots allocated. Keep the row fast path guarded
    // by that original shape; otherwise fall through to the ordinary property site.
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
                    format!(
                        "if (recv_row != NULL && recv_obj->shape == recv_shape) {{ d{v} = d{prev};{box_stmt} handled = 1; }} "
                    ),
                    String::new(),
                )
            } else {
                (
                    format!(
                        "if (recv_row != NULL && recv_obj->shape == recv_shape) {{ v{v} = v{prev}; handled = 1; }} "
                    ),
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
                        "if (recv_row != NULL && recv_obj->shape == recv_shape) {{ memcpy(&d{v}, &recv_obj->slots[recv_row[{i}]], sizeof(double));{box_stmt} handled = 1; }} "
                    ),
                    String::new(),
                )
            } else {
                (
                    format!(
                        "if (recv_row != NULL && recv_obj->shape == recv_shape) {{ v{v} = recv_obj->slots[recv_row[{i}]]; handled = 1; }} "
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
        let slow_read = format!(
            "{{ static kiln_c99_site fallback_site = {{0}}; int status = kiln_c99_val_read_property_key(host, v{o}, {symbol}_key_{name}, depth, thrown, &fallback_site, &v{v}); if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_property_refusal(kiln_val_to_boundary(v{o})); {check} }}",
            o = object.0,
        );
        let int_sync = if typed_int {
            format!(" i{v} = (int64_t)d{v};")
        } else {
            String::new()
        };
        match (this_site, prev) {
            (Some(_), Some(prev_vid)) => {
                if typed {
                    let box_stmt = if need_box {
                        format!(" v{v} = v{prev_vid};")
                    } else {
                        String::new()
                    };
                    writeln!(output, "    if (KILN_UNLIKELY(recv_obj->shape != recv_shape)) goto kiln_refusal; d{v} = d{prev_vid};{box_stmt}{int_sync}").unwrap();
                } else {
                    writeln!(output, "    if (recv_obj->shape == recv_shape) v{v} = v{prev_vid}; else {slow_read}").unwrap();
                }
            }
            (Some(col_idx), None) => {
                if typed {
                    let box_stmt = if need_box {
                        format!(" v{v} = recv_obj->slots[recv_row[{col_idx}]];")
                    } else {
                        String::new()
                    };
                    writeln!(
                        output,
                        "    if (KILN_UNLIKELY(recv_obj->shape != recv_shape)) goto kiln_refusal; memcpy(&d{v}, &recv_obj->slots[recv_row[{col_idx}]], sizeof(double));{box_stmt}{int_sync}"
                    )
                    .unwrap();
                } else {
                    writeln!(
                        output,
                        "    if (recv_obj->shape == recv_shape) v{v} = recv_obj->slots[recv_row[{col_idx}]]; else {slow_read}"
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
    if ctx.plan.semantics == crate::NativeSemantics::Native {
        if let Some(&(obj_lid, ref obj_expr)) = ctx.native_records.value_exprs.get(object) {
            let prop_name = match ctx.program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
                _ => String::new(),
            };
            let all_layouts = crate::collect_program_record_layouts(ctx.program);
            if let Some(layout) = all_layouts.get(obj_lid.0 as usize) {
                if let Some(field) = layout.fields.iter().find(|f| f.name == prop_name) {
                    let field_expr = format!("{obj_expr}.{prop_name}");
                    match field.ty {
                        kiln_ir::NativeFieldType::Number => {
                            if ctx.live_numbers.contains(value) {
                                writeln!(output, "    {field_expr} = d{val};").unwrap();
                            } else if ctx.live_ints.contains(value) {
                                writeln!(output, "    {field_expr} = (double)i{val};").unwrap();
                            } else {
                                writeln!(output, "    {{ double tmp; if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{val}))) memcpy(&tmp, &v{val}, sizeof(tmp)); else tmp = kiln_c99_as_number(kiln_val_to_boundary(v{val})); {field_expr} = tmp; }}").unwrap();
                            }
                        }
                        kiln_ir::NativeFieldType::Int32 => {
                            if ctx.live_ints.contains(value) {
                                writeln!(output, "    {field_expr} = (int32_t)i{val};").unwrap();
                            } else if ctx.live_numbers.contains(value) {
                                writeln!(output, "    {field_expr} = (int32_t)d{val};").unwrap();
                            } else {
                                writeln!(output, "    {{ double tmp; if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{val}))) memcpy(&tmp, &v{val}, sizeof(tmp)); else tmp = kiln_c99_as_number(kiln_val_to_boundary(v{val})); {field_expr} = (int32_t)tmp; }}").unwrap();
                            }
                        }
                        kiln_ir::NativeFieldType::Boolean => {
                            if ctx.booleans.contains(value) {
                                writeln!(output, "    {field_expr} = b{val};").unwrap();
                            } else {
                                writeln!(output, "    {field_expr} = kiln_c99_val_truthy(v{val});")
                                    .unwrap();
                            }
                        }
                        kiln_ir::NativeFieldType::String => {
                            writeln!(output, "    {field_expr} = v{val};").unwrap();
                        }
                        kiln_ir::NativeFieldType::Record(dst_sub_lid) => {
                            if let Some(&(src_sub_lid, ref val_expr)) =
                                ctx.native_records.value_exprs.get(value)
                            {
                                super::emit_struct_copy_fields(
                                    output,
                                    &field_expr,
                                    dst_sub_lid,
                                    val_expr,
                                    src_sub_lid,
                                    &all_layouts,
                                );
                            }
                        }
                        kiln_ir::NativeFieldType::NumberArray => {
                            writeln!(output, "    {field_expr} = v{val};").unwrap();
                        }
                        kiln_ir::NativeFieldType::RecordArray(_) => {
                            if let Some(&(_, ref arr_expr)) =
                                ctx.native_records.record_array_value_exprs.get(value)
                            {
                                writeln!(output, "    {field_expr} = {arr_expr};").unwrap();
                            }
                        }
                    }
                    if is_used {
                        writeln!(output, "    v{v} = v{val};").unwrap();
                        if typed && ctx.live_numbers.contains(value) {
                            writeln!(output, "    d{v} = d{val};").unwrap();
                        }
                        if typed_int && ctx.live_ints.contains(value) {
                            writeln!(output, "    i{v} = i{val};").unwrap();
                        }
                    }
                    return;
                }
            }
        }
    }
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
                    kiln_c99_heap_store_val(obj, &obj->slots[slot], v{val}); \
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
                            kiln_c99_prototype_touched(obj); kiln_c99_heap_store_val(obj, &obj->slots[s0], v{val}); \
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

pub(super) fn emit_define_own_property(
    output: &mut String,
    vid: ValueId,
    object: &ValueId,
    name: &u32,
    value: &ValueId,
    ctx: &OpContext,
) {
    let (pc, label) = ctx
        .origins
        .get(&vid)
        .map(|origin| (origin.pc, origin.label.as_str()))
        .unwrap_or((0, "Op"));
    let check = status_check(ctx.diagnostic_site, pc, label, ctx.unwind);
    writeln!(output,
        "    {{ int status = kiln_c99_create_data_property(host, kiln_val_to_boundary(v{}), kiln_c99_value(KILN_VALUE_STRING, 0, (const char *){}_key_{}, {}_key_{}->unit_len), kiln_val_to_boundary(v{}), depth, thrown); {} v{} = v{}; }}",
        object.0, ctx.symbol, name, ctx.symbol, name, value.0, check, vid.0, value.0)
        .unwrap();
}

pub(super) fn emit_define_own_computed(
    output: &mut String,
    vid: ValueId,
    object: &ValueId,
    key: &ValueId,
    value: &ValueId,
    ctx: &OpContext,
) {
    let (pc, label) = ctx
        .origins
        .get(&vid)
        .map(|origin| (origin.pc, origin.label.as_str()))
        .unwrap_or((0, "Op"));
    let check = status_check(ctx.diagnostic_site, pc, label, ctx.unwind);
    writeln!(output,
        "    {{ int status = kiln_c99_create_data_property(host, kiln_val_to_boundary(v{}), kiln_val_to_boundary(v{}), kiln_val_to_boundary(v{}), depth, thrown); {} v{} = v{}; }}",
        object.0, key.0, value.0, check, vid.0, value.0)
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
    if ctx.plan.semantics == crate::NativeSemantics::Native {
        if let Some(&(lid, ref arr_expr)) = ctx.native_records.record_array_value_exprs.get(object)
        {
            let idx_expr = if ctx.live_ints.contains(key) || ctx.ints.contains(key) {
                format!("(size_t)i{}", key.0)
            } else if ctx.live_numbers.contains(key) || ctx.numbers.contains(key) {
                format!("(size_t)d{}", key.0)
            } else {
                format!(
                    "((size_t)kiln_c99_as_number(kiln_val_to_boundary(v{})))",
                    key.0
                )
            };
            writeln!(output, "    rec_ptr_{v} = &({arr_expr}).data[{idx_expr}];").unwrap();
            if ctx.native_records.boxed_record_values.contains(&vid) {
                let all_layouts = crate::collect_program_record_layouts(program);
                crate::emit_graph::emit_box_native_record(
                    output,
                    ctx.symbol,
                    v,
                    lid,
                    &format!("(*rec_ptr_{v})"),
                    &all_layouts,
                );
            }
            return;
        }
        if ctx.native_records.number_array_values.contains(object) {
            let idx_expr = if ctx.live_ints.contains(key) || ctx.ints.contains(key) {
                format!("(size_t)i{}", key.0)
            } else if ctx.live_numbers.contains(key) || ctx.numbers.contains(key) {
                format!("(size_t)d{}", key.0)
            } else {
                format!(
                    "((size_t)kiln_c99_as_number(kiln_val_to_boundary(v{})))",
                    key.0
                )
            };
            writeln!(output, "    {{ const kiln_array *arr = (const kiln_array *)KILN_VAL_PTR(v{}); v{v} = arr->elements[{idx_expr}]; }}", object.0).unwrap();
            if typed {
                writeln!(output, "    memcpy(&d{v}, &v{v}, sizeof(d{v}));").unwrap();
            }
            if typed_int {
                if typed {
                    writeln!(output, "    i{v} = (int64_t)d{v};").unwrap();
                } else {
                    writeln!(output, "    {{ double tmp_{v}; memcpy(&tmp_{v}, &v{v}, sizeof(tmp_{v})); i{v} = (int64_t)tmp_{v}; }}").unwrap();
                }
            }
            return;
        }
    }
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
            "    {{ static kiln_c99_symbol_cell kiln_key_site; int status; if (KILN_VAL_IS_SYMBOL(v{k})) {{ status = kiln_c99_val_read_symbol(host, v{o}, v{k}, depth, thrown, &kiln_key_site, &v{v}); }} else if (KILN_VAL_IS_STRING(v{k})) {{ status = kiln_c99_val_read_string_key(host, v{o}, v{k}, depth, thrown, &kiln_key_site, &v{v}); }} else {{ status = kiln_c99_val_read_element(host, v{o}, v{k}, depth, thrown, &v{v}); }} if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_computed_refusal(kiln_val_to_boundary(v{o}), kiln_val_to_boundary(v{k})); {check} }}{typed_sync}",
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
    // Both keyed helpers share a site; keep their out-parameter calls as separate statements so
    // hoist_out_params can root and copy each result on its own branch.
    let keyed_body = if key_is_int {
        String::new()
    } else {
        format!(
            "if (!handled && (KILN_VAL_IS_SYMBOL({vk}) || KILN_VAL_IS_STRING({vk}))) {{ \
                static kiln_c99_symbol_cell kiln_key_site; \
                int status; \
                if (KILN_VAL_IS_SYMBOL({vk})) {{ status = kiln_c99_val_read_symbol(host, v{o}, {vk}, depth, thrown, &kiln_key_site, &v{v}); }} \
                else {{ status = kiln_c99_val_read_string_key(host, v{o}, {vk}, depth, thrown, &kiln_key_site, &v{v}); }} \
                if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_computed_refusal(kiln_val_to_boundary(v{o}), kiln_val_to_boundary({vk})); \
                {check} \
                handled = 1; \
            }} ",
            o = object.0,
        )
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
                {keyed_body}\
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
                keyed_body = keyed_body,
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
    let symbol = ctx.symbol;
    let (pc, label) = ctx
        .origins
        .get(&vid)
        .map(|o| (o.pc, o.label.as_str()))
        .unwrap_or((0, "Op"));
    if ctx.plan.semantics == crate::NativeSemantics::Native {
        if let Some(&(lid, ref arr_expr)) = ctx.native_records.record_array_value_exprs.get(object)
        {
            let idx_expr = if ctx.live_ints.contains(key) || ctx.ints.contains(key) {
                format!("(size_t)i{}", key.0)
            } else if ctx.live_numbers.contains(key) || ctx.numbers.contains(key) {
                format!("(size_t)d{}", key.0)
            } else {
                format!(
                    "((size_t)kiln_c99_as_number(kiln_val_to_boundary(v{})))",
                    key.0
                )
            };
            writeln!(
                output,
                "    if ({idx_expr} >= {arr_expr}.capacity) {{ size_t new_cap = {idx_expr} + 1 < 4 ? 4 : ({idx_expr} + 1) * 2; {arr_expr}.data = ({symbol}_rec_{lid} *)realloc({arr_expr}.data, new_cap * sizeof({symbol}_rec_{lid})); {arr_expr}.capacity = new_cap; }}\n    if ({idx_expr} >= {arr_expr}.count) {arr_expr}.count = {idx_expr} + 1;"
            ).unwrap();
            let dst_expr = format!("{arr_expr}.data[{idx_expr}]");
            if let Some(&(src_lid, ref src_expr)) = ctx.native_records.value_exprs.get(value) {
                let all_layouts = crate::collect_program_record_layouts(program);
                emit_struct_copy_fields(output, &dst_expr, lid, src_expr, src_lid, &all_layouts);
            }
            return;
        }
        if ctx.native_records.number_array_values.contains(object) {
            let idx_expr = if ctx.live_ints.contains(key) || ctx.ints.contains(key) {
                format!("(size_t)i{}", key.0)
            } else if ctx.live_numbers.contains(key) || ctx.numbers.contains(key) {
                format!("(size_t)d{}", key.0)
            } else {
                format!(
                    "((size_t)kiln_c99_as_number(kiln_val_to_boundary(v{})))",
                    key.0
                )
            };
            let val_expr = if ctx.live_numbers.contains(value) {
                format!("kiln_c99_val_from_double(d{})", value.0)
            } else if ctx.live_ints.contains(value) {
                format!("kiln_c99_val_from_double((double)i{})", value.0)
            } else {
                format!("v{}", value.0)
            };
            let check = status_check(diagnostic_site, pc, "SetComputed", unwind);
            writeln!(
                output,
                "    {{ kiln_val elem_v = {val_expr}; kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{}); if (KILN_LIKELY({idx_expr} < arr->count)) {{ kiln_c99_heap_store_val(arr, &arr->elements[{idx_expr}], elem_v); }} else {{ int status = kiln_c99_val_write_element(host, v{}, kiln_c99_val_from_double((double)({idx_expr})), elem_v, 1, depth, thrown); {check} }} {assign_v}}}",
                object.0, object.0
            )
            .unwrap();
            if is_used && typed {
                if ctx.live_numbers.contains(value) {
                    writeln!(output, "    d{v} = d{};", value.0).unwrap();
                } else if ctx.live_ints.contains(value) {
                    writeln!(output, "    d{v} = (double)i{};", value.0).unwrap();
                }
            }
            if is_used && typed_int {
                if ctx.live_ints.contains(value) {
                    writeln!(output, "    i{v} = i{};", value.0).unwrap();
                } else if ctx.live_numbers.contains(value) {
                    writeln!(output, "    i{v} = (int64_t)d{};", value.0).unwrap();
                }
            }
            return;
        }
    }
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
            "    {{ static kiln_c99_symbol_cell kiln_key_site; int status; if (KILN_VAL_IS_SYMBOL(v{k})) {{ status = kiln_c99_val_write_symbol(host, v{o}, v{k}, v{val}, {strict}, depth, thrown, &kiln_key_site); }} else if (KILN_VAL_IS_STRING(v{k})) {{ status = kiln_c99_val_write_string_key(host, v{o}, v{k}, v{val}, {strict}, depth, thrown, &kiln_key_site); }} else {{ status = kiln_c99_val_write_element(host, v{o}, v{k}, v{val}, {strict}, depth, thrown); }} {check} {assign_v}}}{typed_sync}",
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
        let (val_d_init, val_guard) = typed_store_value(ctx, value);
        (
            format!(
                "kiln_array *arr = (kiln_array *)ptr; if (KILN_LIKELY({arr_bound} && !arr->frozen)) {{ if (arr->is_numeric && !KILN_VAL_IS_NUMBER(v{val})) arr->is_numeric = 0; kiln_c99_heap_store_val(arr, &arr->elements[i{k}], v{val}); {assign_v}handled = 1; }}",
                k = key.0,
                val = value.0,
                assign_v = assign_v,
            ),
            format!(
                "kiln_typed_array *view = (kiln_typed_array *)ptr; if (KILN_LIKELY({typed_bound}{val_guard})) {{ {val_d_init} kiln_c99_typed_set(view, (size_t)i{k}, val_d); {assign_v}handled = 1; }}",
                k = key.0,
                val_d_init = val_d_init,
                assign_v = assign_v,
            ),
        )
    } else {
        let (val_d_init, val_guard) = typed_store_value(ctx, value);
        (
            format!(
                "double idx; memcpy(&idx, &v{k}, sizeof(idx)); size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; kiln_array *arr = (kiln_array *)ptr; if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < arr->count && !arr->frozen)) {{ if (arr->is_numeric && !KILN_VAL_IS_NUMBER(v{val})) arr->is_numeric = 0; kiln_c99_heap_store_val(arr, &arr->elements[slot], v{val}); {assign_v}handled = 1; }}",
                k = key.0,
                val = value.0,
                assign_v = assign_v,
            ),
            format!(
                "double idx; memcpy(&idx, &v{k}, sizeof(idx)); size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; kiln_typed_array *view = (kiln_typed_array *)ptr; if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < view->length && !view->buffer->detached && view->kind <= KILN_TYPED_FLOAT64{val_guard})) {{ {val_d_init} kiln_c99_typed_set(view, slot, val_d); {assign_v}handled = 1; }}",
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
    let key_site_decl = if key_is_int {
        String::new()
    } else {
        "static kiln_c99_symbol_cell kiln_key_site; ".to_string()
    };
    // Realms issue 17: a symbol key writes through the shape and slot caches a named write uses.
    let symbol_body = if key_is_int {
        String::new()
    } else {
        format!(
            "if (!handled && KILN_VAL_IS_SYMBOL(v{k})) {{ \
                int status = kiln_c99_val_write_symbol(host, v{o}, v{k}, v{val}, {strict}, depth, thrown, &kiln_key_site); \
                {check} \
                {assign_v}\
                handled = 1; \
            }} ",
            k = key.0,
            o = object.0,
            val = value.0,
        )
    };
    let string_body = if key_is_int {
        String::new()
    } else {
        format!(
            "if (!handled && KILN_VAL_IS_STRING(v{k})) {{ \
                int status = kiln_c99_val_write_string_key(host, v{o}, v{k}, v{val}, {strict}, depth, thrown, &kiln_key_site); \
                {check} \
                {assign_v}\
                handled = 1; \
            }} ",
            k = key.0,
            o = object.0,
            val = value.0,
        )
    };
    writeln!(
                output,
                "    {{ int handled = 0; {key_site_decl}{vk_decl}\
                if (KILN_LIKELY((v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX{number_guard})) {{ \
                    void *ptr = KILN_VAL_PTR(v{o}); \
                    uint32_t kind = kiln_c99_gc_payload_kind(ptr); \
                    if (kind == KILN_GC_ARRAY) {{ \
                        {array_body} \
                    }} else if (kind == KILN_GC_TYPED_ARRAY) {{ \
                        {typed_body} \
                    }} \
                }} \
                {symbol_body}\
                {string_body}\
                if (!handled) {{ \
                    int status = kiln_c99_val_write_element(host, v{o}, {vk}, v{val}, {strict}, depth, thrown); \
                    {check} \
                    {assign_v}\
                }} }}\
                {typed_sync}",
                o = object.0,
                val = value.0,
                key_site_decl = key_site_decl,
                vk_decl = vk_decl,
                vk = vk,
                number_guard = number_guard,
                array_body = array_body,
                typed_body = typed_body,
                symbol_body = symbol_body,
                string_body = string_body,
                check = check,
                assign_v = assign_v,
                typed_sync = typed_sync,
            )
            .unwrap();
}

/// The value a typed-array element store's fast path writes, and the condition it adds to that
/// path. A number -- a twin, or a box that is one -- is stored as it stands. Anything else is not
/// the fast path's to convert: `ToNumber` of an object runs its `valueOf`, which can throw or
/// have effects, and a BigInt view takes `ToBigInt`, not a double. Converting either here stored
/// NaN for `{ valueOf() { return 7 } }` without calling it, and `0n` for a BigInt
/// (`BigInt64Array.prototype.set`, parallel-builtins issue 18); the generic element write does
/// both as the language says.
fn typed_store_value(ctx: &OpContext, value: &ValueId) -> (String, String) {
    let val = value.0;
    if ctx.live_numbers.contains(value) {
        (format!("double val_d = d{val};"), String::new())
    } else if ctx.live_ints.contains(value) {
        (format!("double val_d = (double)i{val};"), String::new())
    } else {
        (
            format!("double val_d; memcpy(&val_d, &v{val}, sizeof(val_d));"),
            format!(" && KILN_VAL_IS_NUMBER(v{val})"),
        )
    }
}
