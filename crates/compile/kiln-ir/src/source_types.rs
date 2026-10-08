//! Source contracts annotate bytecode metadata; they never remove runtime guards.
use crate::{Program, TypeAnnotation, TypeAnnotations};
use kiln_hir::{
    Module,
    source_types::{CheckResult, TypeClass, TypeRole},
};
use std::collections::BTreeMap;
/// Metadata format identity, included in the versioned bytecode contract.
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
        // Static TS execution uses generated JavaScript coordinates. Checker signatures use
        // authored coordinates, so cross this boundary through the frontend's source map.
        let signature = if let Some(provenance) = &module.static_source_provenance {
            provenance
                .original_offset(proto.span.start)
                .and_then(|start| signature_starts.get(&start).copied())
        } else {
            signatures.get(&proto.span).copied()
        };
        let Some(signature) = signature else {
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
