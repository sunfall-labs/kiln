//! Source contracts annotate bytecode metadata; they never remove runtime guards.
use crate::{Program, TypeAnnotation, TypeAnnotations};
use kiln_hir::Module;
pub use kiln_hir::source_types::{
    CheckResult, NativeFieldType, RecordField, RecordLayout, RecordLayoutId,
    SOURCE_TYPE_METADATA_VERSION, SourceTypeAnnotations, TypeClass, TypeRole,
};
use std::collections::BTreeMap;
use std::sync::Arc;

fn coarse(class: &TypeClass) -> Option<TypeAnnotation> {
    match class {
        TypeClass::Number => Some(TypeAnnotation::Number),
        TypeClass::Boolean => Some(TypeAnnotation::Boolean),
        TypeClass::String => Some(TypeAnnotation::String),
        TypeClass::Array | TypeClass::Tuple => Some(TypeAnnotation::Array),
        _ => None,
    }
}
/// `KILN_SOURCE_TYPES=0` withholds every checker fact from codegen, so one program can be built
/// with and without them (the A/B switch for type-directed code). With no facts attached,
/// `source_types` stays unset, and the native build identity hashes absent and present facts
/// differently (`kiln-backend-c/src/identity.rs`), so a build that receives them cannot share an
/// artifact with one that does not.
pub fn source_types_enabled() -> bool {
    std::env::var("KILN_SOURCE_TYPES").as_deref() != Ok("0")
}
/// Attach resolved aliases and inferred source contracts to existing bytecode.
/// The executable compiler remains the sole lowering path.
pub fn apply_source_types(
    path: &str,
    module: &Module,
    checked: &CheckResult,
    checker_identity: &str,
    program: &mut Program,
) {
    if !source_types_enabled() || !checked.is_complete_for(path) {
        return;
    }
    // Index this module once. Large projects carry facts for many imported modules;
    // scanning every expression for each function parameter made metadata attachment quadratic.
    let signatures: BTreeMap<_, _> = module
        .type_syntax
        .signatures
        .iter()
        .filter(|signature| signature.has_body)
        .map(|signature| (signature.owner_span, signature))
        .collect();
    let signature_starts: BTreeMap<_, _> = signatures
        .values()
        .map(|signature| (signature.owner_span.start, *signature))
        .collect();
    let signature_ends: BTreeMap<_, _> = signatures
        .values()
        .filter_map(|signature| {
            signature
                .owner_span
                .end
                .checked_sub(1)
                .map(|end| (end, *signature))
        })
        .collect();
    let mut parameter_types = BTreeMap::new();
    let mut parameter_types_by_end = BTreeMap::new();
    let mut parameter_layouts = BTreeMap::new();
    let mut parameter_layouts_by_end = BTreeMap::new();
    let mut parameter_array_elements = BTreeMap::new();
    let mut parameter_array_elements_by_end = BTreeMap::new();
    let mut return_types = BTreeMap::new();
    let mut return_layouts = BTreeMap::new();
    let mut return_array_elements = BTreeMap::new();
    let mut span_layouts = BTreeMap::new();
    let mut exact_span_layouts = BTreeMap::new();
    let mut span_array_elements = BTreeMap::new();
    let mut exact_span_array_elements = BTreeMap::new();

    for fact in checked.types.iter().filter(|fact| fact.path == path) {
        match fact.role {
            TypeRole::Parameter => {
                let coarse_type = coarse(&fact.class);
                parameter_types.entry(fact.span).or_insert(coarse_type);
                parameter_types_by_end
                    .entry(fact.span.end)
                    .or_insert(coarse_type);
                if let Some(layout) = fact.record_layout {
                    parameter_layouts.entry(fact.span).or_insert(layout);
                    parameter_layouts_by_end
                        .entry(fact.span.end)
                        .or_insert(layout);
                }
                if let Some(elem) = fact.array_element {
                    parameter_array_elements.entry(fact.span).or_insert(elem);
                    parameter_array_elements_by_end
                        .entry(fact.span.end)
                        .or_insert(elem);
                }
            }
            TypeRole::Return => {
                return_types
                    .entry(fact.span.start)
                    .or_insert_with(|| coarse(&fact.class));
                if let Some(layout) = fact.record_layout {
                    return_layouts.entry(fact.span.start).or_insert(layout);
                }
                if let Some(elem) = fact.array_element {
                    return_array_elements.entry(fact.span.start).or_insert(elem);
                }
            }
            _ => {}
        }
        if let Some(layout) = fact.record_layout {
            span_layouts.insert(fact.span.start, layout);
            exact_span_layouts.insert((fact.span.start, fact.span.end), layout);
        }
        if let Some(elem) = fact.array_element {
            span_array_elements.insert(fact.span.start, elem);
            exact_span_array_elements.insert((fact.span.start, fact.span.end), elem);
        }
    }
    for proto in &mut program.functions {
        let authored_start = match &module.static_source_provenance {
            Some(provenance) => provenance.original_offset(proto.span.start),
            None => Some(proto.span.start),
        };
        // Static TS execution uses generated JavaScript coordinates. Checker signatures use
        // authored coordinates, so cross this boundary through the frontend's source map.
        let signature = if let Some(provenance) = &module.static_source_provenance {
            provenance
                .original_offset(proto.span.start)
                .and_then(|start| signature_starts.get(&start).copied())
                // A method's executable span includes its key, whereas Oxc's signature span
                // begins at `(`. The source map preserves its closing brace exactly. Match
                // that endpoint rather than guessing which nearby declaration owns the key.
                .or_else(|| {
                    proto
                        .is_method
                        .then(|| proto.span.end.checked_sub(1))
                        .flatten()
                        .and_then(|end| provenance.original_offset(end))
                        .and_then(|end| signature_ends.get(&end).copied())
                })
        } else {
            signatures.get(&proto.span).copied().or_else(|| {
                proto
                    .is_method
                    .then(|| proto.span.end.checked_sub(1))
                    .flatten()
                    .and_then(|end| signature_ends.get(&end).copied())
            })
        };
        let (
            parameters,
            proto_parameter_layouts,
            proto_parameter_array_elements,
            return_type,
            proto_return_layout,
            proto_return_array_element,
        ) = if let Some(signature) = signature {
            let parameters = signature
                .signature
                .parameters
                .iter()
                .map(|parameter| {
                    parameter_types
                        .get(&parameter.span)
                        .copied()
                        .or_else(|| parameter_types_by_end.get(&parameter.span.end).copied())
                        .flatten()
                })
                .collect::<Vec<_>>();
            let proto_parameter_layouts = signature
                .signature
                .parameters
                .iter()
                .map(|parameter| {
                    parameter_layouts
                        .get(&parameter.span)
                        .copied()
                        .or_else(|| parameter_layouts_by_end.get(&parameter.span.end).copied())
                })
                .collect::<Vec<_>>();
            let proto_parameter_array_elements = signature
                .signature
                .parameters
                .iter()
                .map(|parameter| {
                    parameter_array_elements
                        .get(&parameter.span)
                        .copied()
                        .or_else(|| {
                            parameter_array_elements_by_end
                                .get(&parameter.span.end)
                                .copied()
                        })
                })
                .collect::<Vec<_>>();
            let return_type = return_types
                .get(&signature.owner_span.start)
                .or_else(|| authored_start.and_then(|start| return_types.get(&start)))
                .copied()
                .flatten();
            let proto_return_layout = return_layouts
                .get(&signature.owner_span.start)
                .or_else(|| authored_start.and_then(|start| return_layouts.get(&start)))
                .copied();
            let proto_return_array_element = return_array_elements
                .get(&signature.owner_span.start)
                .or_else(|| authored_start.and_then(|start| return_array_elements.get(&start)))
                .copied();
            (
                parameters,
                proto_parameter_layouts,
                proto_parameter_array_elements,
                return_type,
                proto_return_layout,
                proto_return_array_element,
            )
        } else {
            (Vec::new(), Vec::new(), Vec::new(), None, None, None)
        };
        if parameters.iter().any(Option::is_some)
            || return_type.is_some()
            || proto_parameter_layouts.iter().any(Option::is_some)
            || proto_return_layout.is_some()
            || proto_parameter_array_elements.iter().any(Option::is_some)
            || proto_return_array_element.is_some()
            || !span_layouts.is_empty()
            || !span_array_elements.is_empty()
            || !checked.record_layouts.is_empty()
        {
            proto
                .type_annotations
                .get_or_insert_with(TypeAnnotations::default)
                .source_types = Some(SourceTypeAnnotations {
                version: SOURCE_TYPE_METADATA_VERSION,
                checker_identity: checker_identity.into(),
                parameters,
                return_type,
                record_layouts: checked.record_layouts.clone(),
                parameter_layouts: proto_parameter_layouts,
                return_layout: proto_return_layout,
                span_layouts: span_layouts.clone(),
                exact_span_layouts: exact_span_layouts.clone(),
                local_layouts: BTreeMap::new(),
                new_object_layouts: BTreeMap::new(),
                parameter_array_elements: proto_parameter_array_elements,
                return_array_element: proto_return_array_element,
                span_array_elements: span_array_elements.clone(),
                exact_span_array_elements: exact_span_array_elements.clone(),
                local_array_elements: BTreeMap::new(),
                new_array_elements: BTreeMap::new(),
            });
        }
    }

    if !checked.record_layouts.is_empty()
        || !span_layouts.is_empty()
        || !span_array_elements.is_empty()
    {
        let top_annotations = SourceTypeAnnotations {
            version: SOURCE_TYPE_METADATA_VERSION,
            checker_identity: checker_identity.into(),
            parameters: Vec::new(),
            return_type: None,
            record_layouts: checked.record_layouts.clone(),
            parameter_layouts: Vec::new(),
            return_layout: None,
            span_layouts: span_layouts.clone(),
            exact_span_layouts: exact_span_layouts.clone(),
            local_layouts: BTreeMap::new(),
            new_object_layouts: BTreeMap::new(),
            parameter_array_elements: Vec::new(),
            return_array_element: None,
            span_array_elements: span_array_elements.clone(),
            exact_span_array_elements: exact_span_array_elements.clone(),
            local_array_elements: BTreeMap::new(),
            new_array_elements: BTreeMap::new(),
        };
        if let Some(provenance) = &mut program.static_source_provenance {
            Arc::make_mut(provenance).source_types = Some(top_annotations);
        } else {
            program.static_source_provenance = Some(Arc::new(kiln_hir::StaticSourceProvenance {
                original_path: path.into(),
                original_source: module.source.clone(),
                original_type: kiln_hir::StaticSourceType {
                    goal: module.source_goal,
                    commonjs: false,
                    jsx: false,
                },
                transform_fingerprint: "".into(),
                identity: "".into(),
                authored_value_imports: Arc::new([]),
                generated_helper_imports: Arc::new([]),
                authored_internal_helper_imports: Arc::new([]),
                source_map: Arc::new([]),
                source_types: Some(top_annotations),
            }));
        }
    }

    rebind_program_layouts(module, program);
}

/// Rebind local slot layouts and new object layouts on an existing program
/// after compilation or reparse, using the module's HIR body spans.
pub fn rebind_program_layouts(module: &Module, program: &mut Program) {
    let (local_spans, new_object_spans, new_array_spans) =
        crate::compile::collect_body_spans_for_program(module, program);
    let lookup_span_layout =
        |source: &SourceTypeAnnotations, span: kiln_hir::Span| -> Option<RecordLayoutId> {
            let orig_start = module
                .static_source_provenance
                .as_deref()
                .and_then(|prov| prov.original_offset(span.start));
            let orig_end = module
                .static_source_provenance
                .as_deref()
                .and_then(|prov| prov.original_offset(span.end));
            if let (Some(s), Some(e)) = (orig_start, orig_end)
                && let Some(&layout) = source.exact_span_layouts.get(&(s, e))
            {
                return Some(layout);
            }
            if let Some(&layout) = source.exact_span_layouts.get(&(span.start, span.end)) {
                return Some(layout);
            }
            if let Some(s) = orig_start
                && let Some(&layout) = source.span_layouts.get(&s)
            {
                return Some(layout);
            }
            source.span_layouts.get(&span.start).copied()
        };
    let lookup_span_array_element =
        |source: &SourceTypeAnnotations, span: kiln_hir::Span| -> Option<NativeFieldType> {
            let orig_start = module
                .static_source_provenance
                .as_deref()
                .and_then(|prov| prov.original_offset(span.start));
            let orig_end = module
                .static_source_provenance
                .as_deref()
                .and_then(|prov| prov.original_offset(span.end));
            if let (Some(s), Some(e)) = (orig_start, orig_end)
                && let Some(&elem) = source.exact_span_array_elements.get(&(s, e))
            {
                return Some(elem);
            }
            if let Some(&elem) = source
                .exact_span_array_elements
                .get(&(span.start, span.end))
            {
                return Some(elem);
            }
            if let Some(s) = orig_start
                && let Some(&elem) = source.span_array_elements.get(&s)
            {
                return Some(elem);
            }
            source.span_array_elements.get(&span.start).copied()
        };

    if let Some(provenance) = &mut program.static_source_provenance {
        let prov = Arc::make_mut(provenance);
        if let Some(source) = &mut prov.source_types {
            let mut top_local_layouts = std::mem::take(&mut source.local_layouts);
            let mut top_local_array_elements = std::mem::take(&mut source.local_array_elements);
            for (&(fn_span, slot), &span) in &local_spans {
                if fn_span.is_none() {
                    if let Some(layout) = lookup_span_layout(source, span) {
                        top_local_layouts.insert(slot, layout);
                    }
                    if let Some(elem) = lookup_span_array_element(source, span) {
                        top_local_array_elements.insert(slot, elem);
                    }
                }
            }
            let mut top_new_object_layouts = std::mem::take(&mut source.new_object_layouts);
            for (&(fn_span, pc), &span) in &new_object_spans {
                if fn_span.is_none()
                    && let Some(layout) = lookup_span_layout(source, span)
                {
                    top_new_object_layouts.insert(pc, layout);
                }
            }
            let mut top_new_array_elements = std::mem::take(&mut source.new_array_elements);
            for (&(fn_span, pc), &span) in &new_array_spans {
                if fn_span.is_none()
                    && let Some(elem) = lookup_span_array_element(source, span)
                {
                    top_new_array_elements.insert(pc, elem);
                }
            }
            source.local_layouts = top_local_layouts;
            source.new_object_layouts = top_new_object_layouts;
            source.local_array_elements = top_local_array_elements;
            source.new_array_elements = top_new_array_elements;
        }
    }

    for proto in &mut program.functions {
        let func_span = Some(proto.span);
        if let Some(annotations) = &mut proto.type_annotations {
            if let Some(source) = &mut annotations.source_types {
                let prev_local_layouts = std::mem::take(&mut source.local_layouts);
                let prev_new_object_layouts: Vec<RecordLayoutId> =
                    std::mem::take(&mut source.new_object_layouts)
                        .into_values()
                        .collect();
                let mut local_layouts = prev_local_layouts.clone();
                for (slot, param_layout) in source.parameter_layouts.iter().enumerate() {
                    if let Some(id) = param_layout {
                        local_layouts.insert(slot as u32, *id);
                    }
                }
                for (&(fn_span, slot), &span) in &local_spans {
                    if fn_span == func_span
                        && let Some(layout) = lookup_span_layout(source, span)
                    {
                        local_layouts.insert(slot, layout);
                    }
                }
                let func_new_objects: Vec<(u32, kiln_hir::Span)> = new_object_spans
                    .iter()
                    .filter_map(|(&(fn_span, pc), &span)| {
                        (fn_span == func_span).then_some((pc, span))
                    })
                    .collect();
                let mut new_object_layouts = BTreeMap::new();
                for &(pc, span) in &func_new_objects {
                    if let Some(layout) = lookup_span_layout(source, span) {
                        new_object_layouts.insert(pc, layout);
                    }
                }
                if new_object_layouts.is_empty() && !func_new_objects.is_empty() {
                    if func_new_objects.len() == prev_new_object_layouts.len() {
                        for (&(pc, _), &lid) in
                            func_new_objects.iter().zip(prev_new_object_layouts.iter())
                        {
                            new_object_layouts.insert(pc, lid);
                        }
                    } else if func_new_objects.len() == 1 && prev_local_layouts.len() == 1 {
                        if let Some(&lid) = prev_local_layouts.values().next() {
                            new_object_layouts.insert(func_new_objects[0].0, lid);
                        }
                    }
                }
                source.local_layouts = local_layouts;
                source.new_object_layouts = new_object_layouts;

                let prev_local_array_elements = std::mem::take(&mut source.local_array_elements);
                let prev_new_array_elements: Vec<NativeFieldType> =
                    std::mem::take(&mut source.new_array_elements)
                        .into_values()
                        .collect();
                let mut local_array_elements = prev_local_array_elements.clone();
                for (slot, param_elem) in source.parameter_array_elements.iter().enumerate() {
                    if let Some(elem) = param_elem {
                        local_array_elements.insert(slot as u32, *elem);
                    }
                }
                for (&(fn_span, slot), &span) in &local_spans {
                    if fn_span == func_span
                        && let Some(elem) = lookup_span_array_element(source, span)
                    {
                        local_array_elements.insert(slot, elem);
                    }
                }
                let func_new_arrays: Vec<(u32, kiln_hir::Span)> = new_array_spans
                    .iter()
                    .filter_map(|(&(fn_span, pc), &span)| {
                        (fn_span == func_span).then_some((pc, span))
                    })
                    .collect();
                let mut new_array_elements = BTreeMap::new();
                for &(pc, span) in &func_new_arrays {
                    if let Some(elem) = lookup_span_array_element(source, span) {
                        new_array_elements.insert(pc, elem);
                    }
                }
                if new_array_elements.is_empty() && !func_new_arrays.is_empty() {
                    if func_new_arrays.len() == prev_new_array_elements.len() {
                        for (&(pc, _), &elem) in
                            func_new_arrays.iter().zip(prev_new_array_elements.iter())
                        {
                            new_array_elements.insert(pc, elem);
                        }
                    } else if func_new_arrays.len() == 1 && prev_local_array_elements.len() == 1 {
                        if let Some(&elem) = prev_local_array_elements.values().next() {
                            new_array_elements.insert(func_new_arrays[0].0, elem);
                        }
                    }
                }
                source.local_array_elements = local_array_elements;
                source.new_array_elements = new_array_elements;
            }
        }
    }
}
