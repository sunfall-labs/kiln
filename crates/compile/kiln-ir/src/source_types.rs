//! Source contracts annotate bytecode metadata; they never remove runtime guards.
use crate::{Program, TypeAnnotation, TypeAnnotations};
use kiln_hir::{
    Module,
    source_types::{CheckResult, TypeClass, TypeRole},
};
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
    for proto in &mut program.functions {
        let Some(signature) = module
            .type_syntax
            .signatures
            .iter()
            .find(|s| s.has_body && s.owner_span == proto.span)
        else {
            continue;
        };
        let parameters = signature
            .signature
            .parameters
            .iter()
            .map(|p| {
                checked
                    .types
                    .iter()
                    .find(|t| t.path == path && t.role == TypeRole::Parameter && t.span == p.span)
                    .and_then(|t| coarse(&t.class))
            })
            .collect::<Vec<_>>();
        let return_type = checked
            .types
            .iter()
            .find(|t| {
                t.path == path
                    && t.role == TypeRole::Return
                    && t.span.start == signature.owner_span.start
            })
            .and_then(|t| coarse(&t.class));
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
