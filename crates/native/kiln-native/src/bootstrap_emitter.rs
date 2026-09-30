//! The exact backend wrapper used for cached builtin-bootstrap C emission.

use super::NativeError;
use kiln_artifact_cache::Digest;
use kiln_backend_c::{EmissionPolicy, emit_module_with_logical_path_and_policy};
use kiln_ir::ModuleProgram;

pub(super) static EMITTER_ID: std::sync::LazyLock<Digest> = std::sync::LazyLock::new(|| {
    Digest::hash(
        [
            include_str!("bootstrap_emitter.rs").as_bytes(),
            kiln_backend_c::C_MODULE_EMITTER_BUILD_ID.as_bytes(),
        ]
        .concat()
        .as_slice(),
    )
});

pub(super) fn emit(
    program: &ModuleProgram,
    logical_path: &str,
    policy: &EmissionPolicy,
) -> Result<kiln_backend_c::CModule, NativeError> {
    // The bootstrap's top-level closures -- the built-ins it writes in TypeScript -- are made
    // once per process in the shared immutable region (realms issue 08).
    let mut policy = policy.clone();
    policy.shared_region = true;
    Ok(emit_module_with_logical_path_and_policy(
        program,
        logical_path,
        policy,
    )?)
}
