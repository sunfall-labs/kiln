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
    let mut return_types = BTreeMap::new();
    let mut return_layouts = BTreeMap::new();
    let mut span_layouts = BTreeMap::new();
    let mut exact_span_layouts = BTreeMap::new();

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
            }
            TypeRole::Return => {
                return_types
                    .entry(fact.span.start)
                    .or_insert_with(|| coarse(&fact.class));
                if let Some(layout) = fact.record_layout {
                    return_layouts.entry(fact.span.start).or_insert(layout);
                }
            }
            _ => {}
        }
        if let Some(layout) = fact.record_layout {
            span_layouts.insert(fact.span.start, layout);
            exact_span_layouts.insert((fact.span.start, fact.span.end), layout);
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
        let (parameters, proto_parameter_layouts, return_type, proto_return_layout) =
            if let Some(signature) = signature {
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
                let return_type = return_types
                    .get(&signature.owner_span.start)
                    .or_else(|| authored_start.and_then(|start| return_types.get(&start)))
                    .copied()
                    .flatten();
                let proto_return_layout = return_layouts
                    .get(&signature.owner_span.start)
                    .or_else(|| authored_start.and_then(|start| return_layouts.get(&start)))
                    .copied();
                (
                    parameters,
                    proto_parameter_layouts,
                    return_type,
                    proto_return_layout,
                )
            } else {
                (Vec::new(), Vec::new(), None, None)
            };
        if parameters.iter().any(Option::is_some)
            || return_type.is_some()
            || proto_parameter_layouts.iter().any(Option::is_some)
            || proto_return_layout.is_some()
            || !span_layouts.is_empty()
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
            });
        }
    }

    if !checked.record_layouts.is_empty() || !span_layouts.is_empty() {
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
    let (local_spans, new_object_spans) =
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

    if let Some(provenance) = &mut program.static_source_provenance {
        let prov = Arc::make_mut(provenance);
        if let Some(source) = &mut prov.source_types {
            let mut top_local_layouts = BTreeMap::new();
            for (&(fn_span, slot), &span) in &local_spans {
                if fn_span.is_none()
                    && let Some(layout) = lookup_span_layout(source, span)
                {
                    top_local_layouts.insert(slot, layout);
                }
            }
            let mut top_new_object_layouts = BTreeMap::new();
            for (&(fn_span, pc), &span) in &new_object_spans {
                if fn_span.is_none()
                    && let Some(layout) = lookup_span_layout(source, span)
                {
                    top_new_object_layouts.insert(pc, layout);
                }
            }
            source.local_layouts = top_local_layouts;
            source.new_object_layouts = top_new_object_layouts;
        }
    }

    for proto in &mut program.functions {
        let func_span = Some(proto.span);
        if let Some(annotations) = &mut proto.type_annotations {
            if let Some(source) = &mut annotations.source_types {
                let mut local_layouts = BTreeMap::new();
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
                let mut new_object_layouts = BTreeMap::new();
                for (&(fn_span, pc), &span) in &new_object_spans {
                    if fn_span == func_span
                        && let Some(layout) = lookup_span_layout(source, span)
                    {
                        new_object_layouts.insert(pc, layout);
                    }
                }
                source.local_layouts = local_layouts;
                source.new_object_layouts = new_object_layouts;
            }
        }
    }
}
