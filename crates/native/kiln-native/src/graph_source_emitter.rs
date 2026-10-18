//! The complete policy and wrapper boundary for cached Source/CommonJS C emission.
//!
//! Keep this file focused: its digest is part of the persistent generated-C identity. Build
//! orchestration and unrelated graph helpers belong in `lib.rs` and `graph.rs`, not here.

use super::{NativeError, NativeProfile};
use kiln_artifact_cache::Digest;
use kiln_backend_c::{
    EmissionPolicy, emit_commonjs_module_with_logical_path_and_policy,
    emit_module_with_logical_path_and_policy_and_instantiation,
};
use kiln_ir::ModuleProgram;
use kiln_module_graph::ModuleSource;

pub(super) const MAX_RELEASE_SPECIALIZED_MODULE_INSTRUCTIONS: usize = 512;
pub(super) const MAX_RELEASE_SPECIALIZED_MODULE_FUNCTIONS: usize = 16;

fn source_emitter_identity(adapter_source: &[u8], backend_emitter_id: &[u8]) -> Digest {
    Digest::hash([adapter_source, backend_emitter_id].concat().as_slice())
}

/// Changes to this module can change emitted C and must invalidate cached Source/CommonJS C.
pub(super) static SOURCE_EMITTER_ID: std::sync::LazyLock<Digest> = std::sync::LazyLock::new(|| {
    source_emitter_identity(
        include_str!("graph_source_emitter.rs").as_bytes(),
        kiln_backend_c::C_MODULE_EMITTER_BUILD_ID.as_bytes(),
    )
});

/// Whether a generic-only module still lowers its qualifying `parallelMap` sites (issue 13).
///
/// Every profile but the development ones does: a release module goes generic-only for its
/// size (the budget below), which says nothing about its sites, and the lowering costs only
/// where a site exists. The development profiles (`native-dev`, `check`) do not, by decision:
/// they are the edit-run loop, which emits no typed code at all and is compiled for build
/// speed rather than run speed, so a pooled site there would buy little and cost emission and
/// compile time on every edit; the answer is identical either way. Their sites say
/// `dev-profile`, naming `--profile native-release`.
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
        if policy.semantics == kiln_backend_c::NativeSemantics::Native {
            policy.generic_only = false;
            policy.parallel_workers_unit = false;
            policy.unit_chunk_bytes.get_or_insert(1 << 20);
        } else {
            policy.generic_only = release_module_uses_generic_bodies(&node.program, &policy);
            // Issue 14: such a module is compiled at -O0 for build time; its parallel workers go to
            // a unit of their own, compiled like a specialized module (`module_units`).
            policy.parallel_workers_unit = policy.generic_only;
        }
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
    instantiate: bool,
) -> Result<kiln_backend_c::CModule, NativeError> {
    match node.source {
        ModuleSource::Source => Ok(emit_module_with_logical_path_and_policy_and_instantiation(
            &node.program,
            logical_path,
            policy.clone(),
            instantiate,
        )?),
        ModuleSource::CommonJs => Ok(emit_commonjs_module_with_logical_path_and_policy(
            &node.program,
            logical_path,
            policy.clone(),
        )?),
        _ => unreachable!("cached source emitter only accepts Source and CommonJS modules"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_cache_identity_tracks_the_backend_module_emitter() {
        let adapter = include_str!("graph_source_emitter.rs").as_bytes();
        let backend = kiln_backend_c::C_MODULE_EMITTER_BUILD_ID.as_bytes();
        assert_eq!(
            *SOURCE_EMITTER_ID,
            source_emitter_identity(adapter, backend)
        );
        assert_ne!(
            *SOURCE_EMITTER_ID,
            source_emitter_identity(adapter, b"different module emitter")
        );
    }
}
