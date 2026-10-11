/// Mutable static state embedded in the generated-C prelude, classified by its required owner.
///
/// This is the migration manifest for ADR 0036, and since realms issue 04 step 5 (ADR 0070) it has
/// no `Realm` owner: realm state is not a static at all but a field of `kiln_c99_realm_state`
/// (`runtime/foundation.c`), reached through the realm's heap. Adding a mutable `kiln_c99_*`
/// static without naming one of the owners below fails the architecture test, and the owners that
/// are per thread fail it unless the declaration says `_Thread_local`.
pub(super) const C99_MUTABLE_STATE_OWNERS: &[(&str, C99StateOwner)] = &[
    ("kiln_c99_diagnostic", C99StateOwner::Frame),
    ("kiln_c99_diagnostic_active_depth", C99StateOwner::Frame),
    // The key stays on the caller's operand stack until the callee frame consumes this alias.
    (
        "kiln_c99_js_frame_pending_method_name",
        C99StateOwner::ThreadCache,
    ),
    ("kiln_c99_active_http_response", C99StateOwner::Frame),
    ("kiln_c99_new_target", C99StateOwner::Frame),
    ("kiln_c99_inheriting_new_target", C99StateOwner::Frame),
    ("kiln_c99_typed_status", C99StateOwner::Frame),
    ("kiln_c99_typed_thrown", C99StateOwner::Frame),
    ("kiln_c99_adopted_this", C99StateOwner::Frame),
    ("kiln_c99_frame_args", C99StateOwner::Frame),
    ("kiln_c99_frame_argc", C99StateOwner::Frame),
    ("kiln_c99_captured_arguments", C99StateOwner::Frame),
    ("kiln_c99_frame_arguments_object", C99StateOwner::Frame),
    ("kiln_c99_frame_is_arrow", C99StateOwner::Frame),
    ("kiln_c99_frame_mapped_arguments", C99StateOwner::Frame),
    ("kiln_c99_frame_callee", C99StateOwner::Frame),
    // Nested managed launcher calls save their stack/register boundary until the
    // matching pop. The TLS stack is frame bookkeeping, not persistent realm state.
    ("kiln_c99_graph_drive_boundaries", C99StateOwner::Frame),
    ("kiln_c99_ffi_callback_status", C99StateOwner::Frame),
    ("kiln_c99_ffi_callback_host", C99StateOwner::Frame),
    ("kiln_c99_ffi_callback_thrown", C99StateOwner::Frame),
    ("kiln_c99_ffi_callback_depth", C99StateOwner::Frame),
    ("kiln_c99_ffi_callback_active", C99StateOwner::Frame),
    // Counts the engine builtin modules whose synchronous initializer is running; no turn
    // boundary sees it.
    ("kiln_c99_private_hooks_active", C99StateOwner::Frame),
    ("kiln_c99_fixture_method_stores", C99StateOwner::TestFixture),
    ("kiln_c99_gc_fallback", C99StateOwner::TestFixture),
    ("kiln_c99_keys_check_mode", C99StateOwner::TestFixture),
    ("kiln_c99_method_index", C99StateOwner::Constant),
    ("kiln_c99_method_index_len", C99StateOwner::Constant),
    ("kiln_c99_method_index_ready", C99StateOwner::Constant),
    // The surface table's index: the same rows sorted once, a constant once built.
    ("kiln_c99_builtin_member_index", C99StateOwner::Constant),
    ("kiln_c99_builtin_member_index_len", C99StateOwner::Constant),
    // Row indices only, published by the same process-once guard as the member index.
    (
        "kiln_c99_builtin_method_name_index",
        C99StateOwner::Constant,
    ),
    (
        "kiln_c99_builtin_method_name_index_len",
        C99StateOwner::Constant,
    ),
    (
        "kiln_c99_builtin_member_index_ready",
        C99StateOwner::Constant,
    ),
    // Per thread, and trusted only under the realm generation it was filled under.
    ("kiln_c99_realm_cached", C99StateOwner::ThreadCache),
    // ADR 0071: only whether *some* realm has modified *some* built-in; which ones a realm has
    // modified is on its heap, and no cache entry holds a value of this.
    ("kiln_c99_builtin_store_epoch", C99StateOwner::Process),
    // Recycled byte blocks: plain memory that names no realm's value.
    ("kiln_c99_bytes_cache", C99StateOwner::ThreadCache),
    // Parallel issue 30: the lowest address this thread's calls may start at, found once.
    ("kiln_c99_stack_floor", C99StateOwner::ThreadCache),
    // Per thread, and trusted only for the heap serial it was built under: the `node:http` layouts.
    ("kiln_c99_node_http_layouts", C99StateOwner::ThreadCache),
    ("kiln_c99_gc_stress_checks", C99StateOwner::Process),
    // Monotonic ID for candidate-only chunks; the arena uses an atomic increment, and no
    // realm object or graph pointer is stored in this process-wide counter.
    ("kiln_c99_graph_candidate_next_id", C99StateOwner::Process),
    // Set once, never cleared, by the first actor candidate in any realm: a sticky process
    // fact that only ever turns the literal fast path off.
    ("kiln_c99_actor_candidate_ever", C99StateOwner::Process),
    // Parallel issue 06's write detector (`KILN_PARALLEL_PROTECT`), read from the environment once.
    ("kiln_c99_par_protect_state", C99StateOwner::Process),
    // Parallel issue 08's worker arena budget (`KILN_PARALLEL_ARENA_KIB`), read once.
    ("kiln_c99_par_arena_budget_bytes", C99StateOwner::Process),
    // `KILN_PARALLEL_TRACE`: whether each arena join says what it adopted, read once.
    ("kiln_c99_par_trace_state", C99StateOwner::Process),
    ("kiln_c99_introspect_pending", C99StateOwner::Process),
    ("kiln_c99_introspect_armed", C99StateOwner::Process),
    // The process-shared block (ADR 0071): shared keys and the shape root, one per process.
    // Each unit caches the pointer to it; the block itself is the provider archive's.
    ("kiln_c99_shared_block", C99StateOwner::Process),
    ("kiln_c99_speculate_mode", C99StateOwner::Process),
    // ADR 0068: whether this process installs kiln's own members, read from the environment once.
    ("kiln_c99_kiln_extensions_state", C99StateOwner::Process),
    ("kiln_c99_node_stdin_paused", C99StateOwner::Process),
    ("kiln_c99_node_stdin_read_active", C99StateOwner::Process),
    ("kiln_c99_node_stdin_request", C99StateOwner::Process),
    // Function-scope statics are state too (realms issue 05: nothing process-wide unclassified).
    // Derived once from constants; a racing second computation writes the same word.
    ("kiln_c99_method_code_end_cached", C99StateOwner::Constant),
    // `1n` and 2^64-1 as provider handles, made once; the first published is kept.
    ("kiln_c99_bigint_one_handle", C99StateOwner::Constant),
    ("kiln_c99_bigint_u64_mask_handle", C99StateOwner::Constant),
    // Headers for built-in text, claimed and published once each (`kiln_c99_static_text`).
    ("kiln_c99_static_texts", C99StateOwner::Constant),
    // `Symbol()` serials: process-wide so no realm mints one twice.
    ("kiln_c99_symbol_serial", C99StateOwner::Process),
    // Each well-known symbol's shared canonical key, published once (`kiln_c99_symbol_ikey`).
    ("kiln_c99_well_known_ikeys", C99StateOwner::Constant),
    // The `KILN_SPECULATE=exit` test mode's draw, per thread.
    ("kiln_c99_speculate_rng", C99StateOwner::ThreadCache),
    // Which later entry of which site answered this thread last (`kiln_c99_site_answered`).
    ("kiln_c99_streaks", C99StateOwner::ThreadCache),
    // The no-write read path (realms issue 08, parallel-builtins issue 06): whether this thread
    // is reading without writing, around a section that leaves it as it found it.
    ("kiln_c99_no_write_depth", C99StateOwner::Frame),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum C99StateOwner {
    /// A register of the running call, saved and restored around it, so idle at every turn
    /// boundary: per thread (`_Thread_local`), because the machine stack it spills is the
    /// thread's. See `runtime/builtins.c`.
    Frame,
    /// A per-thread cache that is either validated against the realm it was filled in (the realm
    /// generation, a heap serial) or holds nothing any realm made. `_Thread_local`.
    ThreadCache,
    TestFixture,
    /// Derived once from compile-time constants and never changed again, so every realm in the
    /// process would compute the same bytes. Realms on several threads may ask for it first at
    /// once: an index sorted in place is built under `kiln_c99_once`, so nobody searches it
    /// half-sorted; a single word or an entry is published once, by one store or compare-and-swap,
    /// and a racing second computation of it writes the same bytes or loses. A realm neither owns
    /// it nor has to reset it.
    Constant,
    /// Belongs to the process and its signal disposition rather than to any realm, and outlives
    /// every realm the process makes. A realm must not reset it: the `SIGUSR1` handler installed
    /// for the first realm is still installed for the second, and the flag it sets names a
    /// request that has arrived, not a value some realm made.
    Process,
}

impl C99StateOwner {
    /// Whether the owner requires the declaration to be `_Thread_local`.
    pub(super) fn per_thread(self) -> bool {
        matches!(self, Self::Frame | Self::ThreadCache)
    }
}
