//! The exact backend wrapper used for cached builtin-bootstrap C emission.

use super::NativeError;
use kiln_artifact_cache::Digest;
use kiln_backend_c::{EmissionPolicy, emit_module_with_logical_path_and_policy};
use kiln_ir::ModuleProgram;

pub(super) static EMITTER_ID: std::sync::LazyLock<Digest> =
    std::sync::LazyLock::new(|| Digest::hash(include_str!("bootstrap_emitter.rs").as_bytes()));

pub(super) fn emit(
    program: &ModuleProgram,
    logical_path: &str,
    policy: &EmissionPolicy,
) -> Result<kiln_backend_c::CModule, NativeError> {
    Ok(emit_module_with_logical_path_and_policy(
        program,
        logical_path,
        policy.clone(),
    )?)
}
