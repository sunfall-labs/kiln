use super::OpContext;
use super::status_check;
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
        if let Some(&field_idx) = scalar_obj.field_by_name_index.get(name) {
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
    let key_str = match program.constants.get(*name as usize) {
        Some(Constant::String(s)) => s.as_str(),
        _ => "",
    };
    let key_len = key_str.len();
    let check = status_check(diagnostic_site, pc, label, unwind);
    let length_check = if key_str == "length" && spec.arrays.contains(object) {
        format!(
            "{{ v{v} = kiln_c99_val_from_double((double)((const kiln_array *)KILN_VAL_PTR(v{o}))->count); handled = 1; }} ",
            o = object.0
        )
    } else if key_str == "length" {
        format!(
            "if (KILN_LIKELY((v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (KILN_LIKELY(site.has_cached_method == 1 || kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY)) {{ \
                             site.has_cached_method = 1; \
                             v{v} = kiln_c99_val_from_double((double)((const kiln_array *)ptr)->count); \
                             handled = 1; \
                         }} \
                     }} else if (KILN_VAL_IS_STRING(v{o})) {{ \
                         const kiln_string *str = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         v{v} = kiln_c99_val_from_double((double)str->length); \
                         handled = 1; \
                     }} ",
            o = object.0,
            v = v
        )
    } else {
        String::new()
    };
    let size_check = if key_str == "size" {
        format!(
            "if ((v{o} & KILN_TAG_MASK) == KILN_TAG_OBJECT_PREFIX || (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {{ \
                             const kiln_collection *col = (const kiln_collection *)ptr; \
                             if (!col->is_weak) {{ \
                                 v{v} = kiln_c99_val_from_double((double)col->live); \
                                 handled = 1; \
                             }} \
                         }} \
                     }} ",
            o = object.0,
            v = v
        )
    } else {
        String::new()
    };
    // A read of `this`: the function's receiver-shape cache answers first, one indexed
    // load and no compare, because the shape was matched once at entry. The boxed
    // clone, whose slow path is the one that learns, fills the row afterwards.
    // A read of `this`: the function's receiver-shape cache answers with one indexed
    // load when the receiver's shape has a row, which was filled whole from the shape.
    let (receiver_tier, receiver_learn) = match (this_site, self_index) {
        (Some(_), Some(_)) if ctx.this_copy.is_some() => (
            format!(
                "if (recv_row != NULL) {{ v{v} = v{prev}; handled = 1; }} ",
                prev = ctx.this_copy.unwrap().0
            ),
            String::new(),
        ),
        (Some(i), Some(_)) => (
            format!(
                "if (recv_row != NULL) {{ v{v} = recv_obj->slots[recv_row[{i}]]; handled = 1; }} "
            ),
            String::new(),
        ),
        _ => (String::new(), String::new()),
    };
    writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; \
                int handled = 0; \
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
                {typed_sync}",
                o = object.0,
                typed_sync = if ctx.ints.contains(&vid) {
                    // An array's length, proven an integer: exact below 2^53, so the cast is the value.
                    format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v})); i{v} = (int64_t)d{v};")
                } else if typed {
                    format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));")
                } else {
                    String::new()
                }
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
        Some(Constant::String(s)) => s.as_str(),
        _ => "",
    };
    let key_len = key_str.len();
    let strict = matches!(op, GraphOp::SetPropertyStrict { strict: true, .. }) as u8;
    let check = status_check(diagnostic_site, pc, label, unwind);
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
        if let Some(&field_idx) = scalar_obj.fields.get(key_str) {
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
    writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; \
                int handled = 0; {array_tier}\
                if (!handled && KILN_LIKELY((v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_NUMBER(v{k}))) {{ \
                    void *ptr = KILN_VAL_PTR(v{o}); \
                    if (site.has_cached_method == 1 || kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                        site.has_cached_method = 1; \
                        double idx; \
                        memcpy(&idx, &v{k}, sizeof(idx)); \
                        size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; \
                        const kiln_array *arr = (const kiln_array *)ptr; \
                        if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < arr->count && arr->elements[slot] != KILN_VAL_HOLE)) {{ \
                            v{v} = arr->elements[slot]; \
                            handled = 1; \
                        }} \
                    }} \
                }} else if (KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_NUMBER(v{k})) {{ \
                    double idx; \
                    memcpy(&idx, &v{k}, sizeof(idx)); \
                    size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; \
                    const kiln_string *str = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                    if (KILN_LIKELY(str != NULL && idx >= 0.0 && idx == (double)slot && slot < str->length && str->length == str->byte_len)) {{ \
                        v{v} = kiln_c99_val_single_char((unsigned char)str->bytes[slot]); \
                        handled = 1; \
                    }} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_read_element(host, v{o}, v{k}, depth, thrown, &v{v}); \
                    if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_computed_refusal(kiln_val_to_boundary(v{o}), kiln_val_to_boundary(v{k})); \
                    {check} \
                }} }}\
                {typed_sync}",
                o = object.0,
                k = key.0,
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
        if let Some(&field_idx) = scalar_obj.fields.get(key_str) {
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
