//! Source contracts annotate bytecode metadata; they never remove runtime guards.
use crate::{Program, TypeAnnotation, TypeAnnotations};
use kiln_hir::{
    Module,
    source_types::{CheckResult, TypeClass, TypeRole},
};
use std::collections::BTreeMap;
/// Metadata format identity, included in bytecode contract version 8.
pub const SOURCE_TYPE_METADATA_VERSION: u16 = 1;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTypeAnnotations {
    pub version: u16,
    pub checker_identity: String,
    pub parameters: Vec<Option<TypeAnnotation>>,
    pub return_type: Option<TypeAnnotation>,
}
fn coarse(class: &TypeClass) -> Option<TypeAnnotation> {
    match class {
        TypeClass::Number => Some(TypeAnnotation::Number),
        TypeClass::Boolean => Some(TypeAnnotation::Boolean),
        TypeClass::String => Some(TypeAnnotation::String),
        _ => None,
    }
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
    if !checked.complete {
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
    let mut parameter_types = BTreeMap::new();
    let mut return_types = BTreeMap::new();
    for fact in checked.types.iter().filter(|fact| fact.path == path) {
        match fact.role {
            TypeRole::Parameter => {
                parameter_types
                    .entry(fact.span)
                    .or_insert_with(|| coarse(&fact.class));
            }
            TypeRole::Return => {
                return_types
                    .entry(fact.span.start)
                    .or_insert_with(|| coarse(&fact.class));
            }
            _ => {}
        }
    }
    for proto in &mut program.functions {
        let Some(signature) = signatures.get(&proto.span) else {
            continue;
        };
        let parameters = signature
            .signature
            .parameters
            .iter()
            .map(|parameter| parameter_types.get(&parameter.span).copied().flatten())
            .collect::<Vec<_>>();
        let return_type = return_types
            .get(&signature.owner_span.start)
            .copied()
            .flatten();
        if parameters.iter().any(Option::is_some) || return_type.is_some() {
            proto
                .type_annotations
                .get_or_insert_with(TypeAnnotations::default)
                .source_types = Some(SourceTypeAnnotations {
                version: SOURCE_TYPE_METADATA_VERSION,
                checker_identity: checker_identity.into(),
                parameters,
                return_type,
            });
        }
    }
}
