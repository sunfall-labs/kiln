//! Backend-neutral value graph construction and analysis over compiled bytecode.

pub mod arguments;
pub mod closed_world;
pub mod demanded;
mod dominators;
pub mod graph;
pub mod summary;
mod suspended_liveness;
pub mod validation;
pub mod verifier;

pub use arguments::{ArgumentsAnalysis, ArgumentsLengthFact, ArrowArgumentsCaptureFacts};
pub use closed_world::{
    closed_world_function_liveness, closed_world_module_live_slots,
    closed_world_module_live_slots_with_declarations, closed_world_namespace_members,
    closed_world_namespace_members_for_slots, retained_module_instructions, retained_module_size,
};
pub use demanded::{
    CORE_METHOD_GROUPS, DemandedMembers, PRUNABLE_METHOD_GROUPS, analyze_demanded_members,
    format_demanded_members, format_retained_groups, method_group_names, method_groups,
};
pub use graph::*;
pub use kiln_db::summaries_digest;
pub use summary::{summarize_function, summarize_graph, summarize_program};
pub use suspended_liveness::suspended_local_liveness;
pub use validation::{StackEffectError, stack_effect};
pub use verifier::{VerifierError, verify_function_graph};

pub fn instruction_diagnostic(
    instruction: &kiln_ir::Instruction,
    constants: &[kiln_ir::Constant],
) -> String {
    let debug = format!("{instruction:?}");
    let mut operation = debug
        .split([' ', '(', '{'])
        .next()
        .unwrap_or("Instruction")
        .to_owned();
    let named = match instruction {
        kiln_ir::Instruction::CallMethod { name, .. }
        | kiln_ir::Instruction::CallMethodPrepared { name, .. }
        | kiln_ir::Instruction::GetProperty(name)
        | kiln_ir::Instruction::SetProperty(name)
        | kiln_ir::Instruction::DefineOwnProperty(name)
        | kiln_ir::Instruction::SetPropertyStrict { name, .. } => constants.get(*name as usize),
        _ => None,
    };
    if let Some(kiln_ir::Constant::String(name)) = named {
        operation.push('(');
        operation.push_str(&name.to_scalar_string().unwrap_or_default());
        operation.push(')');
    }
    operation
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric() || b"_.$:-()@".contains(byte))
        .map(char::from)
        .collect()
}
