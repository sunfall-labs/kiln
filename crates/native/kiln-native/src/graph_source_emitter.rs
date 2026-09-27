//! The complete policy and wrapper boundary for cached Source/CommonJS C emission.
//!
//! Keep this file focused: its digest is part of the persistent generated-C identity. Build
//! orchestration and unrelated graph helpers belong in `lib.rs` and `graph.rs`, not here.

use super::*;

pub(super) const MAX_RELEASE_SPECIALIZED_MODULE_INSTRUCTIONS: usize = 512;
pub(super) const MAX_RELEASE_SPECIALIZED_MODULE_FUNCTIONS: usize = 16;

/// Changes to this module can change emitted C and must invalidate cached Source/CommonJS C.
pub(super) static SOURCE_EMITTER_ID: std::sync::LazyLock<Digest> =
    std::sync::LazyLock::new(|| Digest::hash(include_str!("graph_source_emitter.rs").as_bytes()));

pub(super) fn parallel_in_generic(profile: NativeProfile) -> bool {
    !matches!(profile, NativeProfile::Dev | NativeProfile::Check)
}

pub(super) fn release_module_uses_generic_bodies(
    module: &ModuleProgram,
    policy: &EmissionPolicy,
) -> bool {
    let (instructions, functions) =
        kiln_backend_c::retained_module_size(module, policy.live_module_slots.as_deref());
    functions > MAX_RELEASE_SPECIALIZED_MODULE_FUNCTIONS
        || instructions > MAX_RELEASE_SPECIALIZED_MODULE_INSTRUCTIONS
}

/// Return the exact policy used by the cached Source/CommonJS emitter for this profile.
pub(super) fn effective_policy(
    node: &kiln_module_graph::ModuleNode,
    policy: &EmissionPolicy,
    profile: NativeProfile,
) -> EmissionPolicy {
    let mut policy = policy.clone();
    policy.parallel_in_generic = parallel_in_generic(profile);
    if profile == NativeProfile::Release {
        policy.generic_only = release_module_uses_generic_bodies(&node.program, &policy);
        // Issue 14: such a module is compiled at -O0 for build time; its parallel workers go to
        // a unit of their own, compiled like a specialized module (`module_units`).
        policy.parallel_workers_unit = policy.generic_only;
    } else if profile == NativeProfile::ReleaseSize {
        policy.generic_only = true;
        policy.omit_source = true;
    }
    policy.compact = profile != NativeProfile::Profile;
    policy
}

/// The single miss path for cached Source/CommonJS modules. Keep these calls in this file so
/// the fingerprint above names the exact code that produces the cached payload.
pub(super) fn emit(
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &EmissionPolicy,
) -> Result<kiln_backend_c::CModule, NativeError> {
    match node.source {
        ModuleSource::Source => Ok(emit_module_with_logical_path_and_policy(
            &node.program,
            logical_path,
            policy.clone(),
        )?),
        ModuleSource::CommonJs => Ok(emit_commonjs_module_with_logical_path_and_policy(
            &node.program,
            logical_path,
            policy.clone(),
        )?),
        _ => unreachable!("cached source emitter only accepts Source and CommonJS modules"),
    }
}
