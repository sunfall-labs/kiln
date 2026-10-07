//! Deterministic C99 emission for Kiln's scalar bytecode.
//!
//! This crate is deliberately a backend, not a C runtime.  The generated translation unit owns
//! the value representation and calls a host callback for every [`kiln_ir::Instruction::HostCall`]
//! using the numeric [`kiln_host_abi::HostOp`] value.  In particular, it never writes stdout (or
//! otherwise bakes CLI policy into an object file).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::Write as _,
    sync::Arc,
};

use kiln_host_abi::{
    HOST_ABI_VERSION, HOST_OP_MODULE_CREATE_REQUIRE, HOST_OP_MODULE_LOADER, HostOp,
};
use kiln_ir::{
    BYTECODE_VERSION, BinaryOp, Constant, FunctionProto, Instruction, ModuleProgram, Program,
    UnaryOp, Upvalue,
};
use kiln_native_provider_contract::number::{NUMBER_FORMAT_VERSION, NUMBER_PARSE_VERSION};

/// Version of the generated C ABI.  This is independent from the bytecode and HostOps versions.
pub const C_ABI_VERSION: u32 = 1;

/// Internal host callback code for realm-owned `createRequire` values. The emitted C macro and
/// generated launchers mirror the typed host-ABI authority rather than defining another number.
pub const C99_HOST_MODULE_CREATE_REQUIRE: u16 = HOST_OP_MODULE_CREATE_REQUIRE;
pub const C99_HOST_MODULE_LOADER: u16 = HOST_OP_MODULE_LOADER;

/// Revision of the complete generated translation-unit format.
///
/// This is deliberately independent from [`C_ABI_VERSION`]: diagnostics and private helper code
/// can change emitted object bytes without changing the public embedding layout. Native artifact
/// keys include this revision so those objects are never reused across generator changes.
// Epoch 31 changed the private dictionary-cache index encoding. Epoch 39 indexes SSA values in
// compact bodies once they reach 512 entries; earlier objects used distinct locals until 4,096.
// Epoch 40 omits unused context loads from compact function prologues.
// Epoch 41 removes phi-only chains with no consumers before C emission.
// Epoch 42 resolves trivial phis and densely renumbers surviving SSA values.
// Epoch 43 prunes unreachable declaration bodies; epoch 44 outlines compact operation plumbing;
// epoch 45 packs source ranges observable through live closures; epoch 46 decouples compact C
// source from generic-only emission. Epoch 47 preserves inherited class evaluation and budgets
// release specialization using only retained code. Epoch 48 carries the canonical unit-string
// value bridge and its provider-boundary codecs through generated C. Epoch 49 includes source
// coordinates at generated call and construction sites for native stack frames.
pub const C_GENERATOR_VERSION: u32 = 49;

/// Content identity of this emitter checkout, including dirty source changes. Input-keyed
/// caches must not trust a manually advanced generator epoch alone across active worktrees.
pub const C_EMITTER_BUILD_ID: &str = env!("KILN_C_EMITTER_BUILD_ID");

/// Identity of code that can change generated module C, excluding separately linked runtime C.
/// Runtime and prelude bytes remain inputs to the compiled-object cache keys in `kiln-native`.
pub const C_MODULE_EMITTER_BUILD_ID: &str = env!("KILN_C_MODULE_EMITTER_BUILD_ID");

/// Stable basename used by decoupled translation units for the shared runtime declarations.
///
/// The native pipeline materializes this header once beside every set of generated module files.
/// Keeping the name here makes the emitted include and the materialized file one contract.
pub const RUNTIME_PRELUDE_HEADER_NAME: &str = "kiln_runtime_prelude.h";

/// Release specialization is for small hot bodies. Cloning larger bodies multiplies C parsing,
/// optimization, object size, and instruction-cache pressure for negligible benefit.
pub(crate) const MAX_SPECIALIZED_INSTRUCTIONS: usize = 192;
const COMPACT_DIAGNOSTIC_SITE: &str = "\0kiln-unit-site";

pub(crate) fn diagnostic_source(site: &str) -> String {
    if site == COMPACT_DIAGNOSTIC_SITE {
        "ku_site".to_owned()
    } else {
        c_string(site)
    }
}

pub(crate) fn compact_diagnostic_site(site: &str) -> bool {
    site == COMPACT_DIAGNOSTIC_SITE
}

/// Realm policy that changes observable native execution.
///
/// The default deliberately preserves every pre-ADR-0031 artifact. Host-backed values are never
/// captured here; only the policy participates in emission and identity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EmissionPolicy {
    pub host_clock: bool,
    pub entropy: EmissionEntropyPolicy,
    pub timezone: EmissionStringPolicy,
    pub locale: EmissionStringPolicy,
    pub decoupled_prelude: bool,
    /// Prefer terse generated C and shared helpers where that does not change optimization.
    pub compact: bool,
    /// Emit only generic bodies, without speculative, typed, fast, or parallel clones.
    pub generic_only: bool,
    /// With `generic_only`: still give each `parallelMap` site whose callback qualifies its
    /// parallel lowering -- the callback's typed entry and parallel clone, and the pooled path at
    /// the site -- and nothing else (issue 13). The release profiles set it: a module is emitted
    /// generic-only for its size, which says nothing about its sites. The development profile
    /// does not: its sites say `dev-profile`.
    pub parallel_in_generic: bool,
    /// With `generic_only` and a decoupled prelude: emit the parallel clones and chunk workers
    /// of the qualifying sites into [`CModule::parallel_workers`], a unit of their own, instead
    /// of into the module (issue 14). `kiln-native` sets it for a release module that went
    /// generic-only for its size and compiles that unit at `-O2` while the module stays at
    /// `-O0`. A caller that sets it must compile and link the extra unit.
    pub parallel_workers_unit: bool,
    /// With `compact`: split the module's functions into translation units ([`CModule::chunks`])
    /// of about this many bytes of C each, in function order, whatever its function count.
    /// `kiln-native` sets it for the specialized builtin bootstrap of a release build (issue
    /// 17), whose one-time optimising compile is then parallel units rather than one long one.
    /// `None` keeps the count rule (a module past 500 functions, 1,000 or 1,500 per unit).
    pub unit_chunk_bytes: Option<usize>,
    /// Whether speculative versioning is enabled. Native orchestration resolves its process
    /// setting before emission; `None` preserves the direct emitter's default.
    pub speculate: Option<bool>,
    pub type_profile_record: bool,
    pub type_profile: Option<std::sync::Arc<graph::TypeProfile>>,
    /// Module slots whose exported values can be observed by this closed-world executable.
    /// `None` preserves the public module emitter's complete export surface.
    pub live_module_slots: Option<Arc<BTreeSet<u32>>>,
    /// Explicit regex matcher compilation setting. If `None`, defaults to `!compact`.
    pub compile_regex: Option<bool>,
    /// Omit embedded JavaScript source text for closures (Function.prototype.toString returns [native code]).
    pub omit_source: bool,
    /// Build a binary that never installs kiln's own members (ADR 0068), whatever its
    /// `KILN_EXTENSIONS` says at run time: `kiln build --no-kiln-extensions`.
    pub no_kiln_extensions: bool,
    /// Make each closure the module makes once per realm -- at its top level, or in a function
    /// the top level calls once -- once per process in the shared immutable region, and hand
    /// every realm that one (ADR 0070 primitive B; realms issue 08). Such a closure reads the
    /// bindings it would have captured through the realm's unit block instead (`module_cells`).
    /// `kiln-native` sets it for the runtime bootstrap, whose top level is the built-ins it
    /// writes in TypeScript. The
    /// closure's identity is shared -- no realm can hold another's to compare -- and its
    /// prototype and property store are each realm's own.
    pub shared_region: bool,
}

impl EmissionPolicy {
    pub fn should_compile_regex(&self) -> bool {
        self.compile_regex.unwrap_or(!self.compact)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmissionEntropyPolicy {
    #[default]
    Deterministic,
    Seeded(u64),
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EmissionStringPolicy {
    #[default]
    Deterministic,
    Named(String),
    Host,
}

const _: () = assert!(NUMBER_FORMAT_VERSION == 1);
const _: () = assert!(NUMBER_PARSE_VERSION == 1);

// Keep this declaration beside the provider ABI row rather than maintaining a second C-only
// spelling in the header below. The callback deliberately emits only the one provider entry that
// needs a C declaration here; all other private provider declarations are either not used by this
// backend's generated C or are declared in their feature-specific sections.
/// The C declaration for every private provider entry point the generated program calls.
///
/// One arm per *signature*, which is the same token `assert_provider_export!` uses to pin the Rust
/// type. A C declaration and the implementation it describes therefore come from one registry
/// entry, and a declaration that disagrees with its implementation has nowhere to be written.
/// Adding an export to `provider_abi_registry!` without an arm here declares nothing, and the
/// generated C then fails to compile naming the symbol -- but only because every site that
/// compiles it passes `-Werror=implicit-function-declaration`. Without that flag C is happy to
/// invent an `int f()` for an undeclared call and link something whose ABI nobody checked, which
/// is how the hand-written declarations this replaced went unenforced for their whole life:
/// deleting one changed nothing that any test could see.
macro_rules! provider_c_declaration {
    ($(($group:ident, $name:ident, $signature:ident);)*) => {
        concat!($(provider_c_declaration!($group, $name, $signature)),*)
    };
    ($group:ident, $name:ident, node_path_part) => {
        concat!("int ", stringify!($name), "(uint32_t part, const uint8_t *path, uint64_t path_len, const uint8_t *suffix, uint64_t suffix_len, uint8_t *out, uint64_t capacity, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, node_path_relative) => {
        concat!("int ", stringify!($name), "(const uint8_t *from, uint64_t from_len, const uint8_t *to, uint64_t to_len, uint8_t *out, uint64_t capacity, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, url_parse) => {
        concat!("int ", stringify!($name), "(const uint8_t *input, uint64_t input_len, const uint8_t *base, uint64_t base_len, uint8_t *out, uint64_t capacity, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, text_decoder_label) => {
        concat!("int ", stringify!($name), "(const uint8_t *label, uint64_t label_len, uint8_t *out, uint64_t capacity, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, text_decode) => {
        concat!("int ", stringify!($name), "(const uint8_t *name, uint64_t name_len, const uint8_t *bytes, uint64_t bytes_len, uint32_t flags, uint8_t *out, uint64_t capacity, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, text_decoder_state_create) => {
        concat!("int ", stringify!($name), "(const uint8_t *label, uint64_t label_len, uint32_t flags, void **out_state, uint8_t *out, uint64_t capacity, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, text_decoder_state_decode) => {
        concat!("int ", stringify!($name), "(void *state, const uint8_t *bytes, uint64_t bytes_len, uint32_t stream, uint8_t *out, uint64_t capacity, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, text_decoder_state_free) => {
        concat!("void ", stringify!($name), "(void *state);\n")
    };
    ($group:ident, $name:ident, string_case) => {
        concat!("int ", stringify!($name), "(const uint16_t *units, uint64_t count, uint32_t upper, uint16_t *out, uint64_t capacity, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, string_normalize) => {
        concat!("int ", stringify!($name), "(const uint16_t *units, uint64_t count, uint32_t form, uint16_t *out, uint64_t capacity, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, intl_collate) => {
        concat!("int ", stringify!($name), "(const uint16_t *left, uint64_t left_len, const uint16_t *right, uint64_t right_len, const uint8_t *spec, uint64_t spec_len, int32_t *out_order);\n")
    };
    ($group:ident, $name:ident, intl_string_case) => {
        concat!("int ", stringify!($name), "(const uint16_t *units, uint64_t count, const uint8_t *locale, uint64_t locale_len, uint32_t uppercase, uint16_t *out, uint64_t capacity, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, intl_datetime_format) => {
        concat!("int ", stringify!($name), "(const uint8_t *spec, uint64_t spec_len, double epoch_milliseconds, uint32_t operation, const uint8_t **out, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, url_set) => {
        concat!("int ", stringify!($name), "(const uint8_t *href, uint64_t href_len, uint32_t field, const uint8_t *value, uint64_t value_len, uint8_t *out, uint64_t capacity, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, node_fs_read_file_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, uint8_t **out_bytes, uint64_t *out_len, int32_t *out_error_code);\n")
    };
    ($group:ident, $name:ident, node_fs_free_bytes) => {
        concat!("void ", stringify!($name), "(uint8_t *bytes, uint64_t len);\n")
    };
    ($group:ident, $name:ident, node_fs_write_file_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, const uint8_t *data, uint64_t data_len, int32_t *out_error_code);\n")
    };
    ($group:ident, $name:ident, node_fs_mkdir_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, int32_t recursive, int32_t *out_error_code);\n")
    };
    ($group:ident, $name:ident, node_fs_stat_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, int32_t follow_symlinks, uint64_t *out_size, uint32_t *out_flags, double *out_mtime_ms, uint32_t *out_mode, int32_t *out_error_code);\n")
    };
    ($group:ident, $name:ident, node_fs_readdir_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, uint8_t **out_entries, uint64_t *out_len, uint64_t *out_count, int32_t *out_error_code);\n")
    };
    ($group:ident, $name:ident, node_fs_realpath_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, uint8_t **out_bytes, uint64_t *out_len, int32_t *out_error_code);\n")
    };
    ($group:ident, $name:ident, node_fs_unlink_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, int32_t *out_error_code);\n")
    };
    ($group:ident, $name:ident, node_fs_rmdir_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, int32_t *out_error_code);\n")
    };
    ($group:ident, $name:ident, ffi_open) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, uint64_t *out_library);\n")
    };
    ($group:ident, $name:ident, ffi_close) => {
        concat!("int ", stringify!($name), "(uint64_t library);\n")
    };
    ($group:ident, $name:ident, ffi_prepare) => {
        concat!("int ", stringify!($name), "(uint64_t library, const uint8_t *symbol, uint64_t symbol_len, const uint32_t *argument_types, uint64_t argument_count, uint32_t result_type, uint64_t *out_function);\n")
    };
    ($group:ident, $name:ident, ffi_call) => {
        concat!("int ", stringify!($name), "(uint64_t function, const kiln_ffi_value *arguments, uint64_t argument_count, kiln_ffi_value *out_result);\n")
    };
    ($group:ident, $name:ident, ffi_register_callback) => {
        concat!("int ", stringify!($name), "(const uint32_t *argument_types, uint64_t argument_count, uint32_t result_type, kiln_ffi_dispatch_fn dispatch, void *context, uint64_t *out_pointer);\n")
    };
    ($group:ident, $name:ident, ffi_unregister_callback) => {
        concat!("int ", stringify!($name), "(uint64_t pointer);\n")
    };
    ($group:ident, $name:ident, ffi_last_error) => {
        concat!("int ", stringify!($name), "(const uint8_t **out_bytes, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, realm_argv_at) => {
        concat!("int ", stringify!($name), "(uint64_t index, const uint8_t **out_bytes, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, realm_argv_count) => {
        concat!("int ", stringify!($name), "(uint64_t *out_count);\n")
    };
    ($group:ident, $name:ident, realm_cancel_active_request) => {
        concat!("int ", stringify!($name), "(uint64_t request);\n")
    };
    ($group:ident, $name:ident, realm_cancel_active_request_if_pending) => {
        concat!("int ", stringify!($name), "(uint64_t request);\n")
    };
    ($group:ident, $name:ident, realm_cancel_request) => {
        concat!("int ", stringify!($name), "(uint64_t realm, uint64_t request);\n")
    };
    ($group:ident, $name:ident, realm_clear_timer) => {
        concat!("int ", stringify!($name), "(uint64_t id);\n")
    };
    ($group:ident, $name:ident, realm_rearm_timer) => {
        concat!(
            "int ",
            stringify!($name),
            "(uint64_t id, double delay_ms, kiln_realm_work_fn run, kiln_realm_drop_fn drop, void *work);\n"
        )
    };
    ($group:ident, $name:ident, realm_set_timer_ref) => {
        concat!("int ", stringify!($name), "(uint64_t id, int referenced);\n")
    };
    ($group:ident, $name:ident, realm_create) => {
        concat!("int ", stringify!($name), "(uint64_t *out_realm);\n")
    };
    ($group:ident, $name:ident, realm_destroy) => {
        concat!("int ", stringify!($name), "(uint64_t realm);\n")
    };
    ($group:ident, $name:ident, realm_diagnostic_note) => {
        concat!("int ", stringify!($name), "(const uint8_t *site, uint64_t site_len, uint64_t instruction, const uint8_t *operation, uint64_t operation_len, uint64_t depth, const uint8_t *detail, uint64_t detail_len, const uint8_t *function, uint64_t function_len, uint64_t method, uint64_t receiver_tag, int has_method);\n")
    };
    ($group:ident, $name:ident, realm_diagnostic_render) => {
        concat!("int ", stringify!($name), "(uint8_t *out, uint64_t capacity, uint64_t *out_len);\n")
    };
    ($group:ident, $name:ident, realm_diagnostic_reset) => {
        concat!("int ", stringify!($name), "(void);\n")
    };
    ($group:ident, $name:ident, realm_drive) => {
        concat!("int ", stringify!($name), "(uint64_t realm, void *host, void *thrown);\n")
    };
    ($group:ident, $name:ident, realm_drive_step) => {
        concat!("int ", stringify!($name), "(uint64_t realm, void *host, void *thrown, uint32_t *kind, uint64_t *delay_ns);\n")
    };
    ($group:ident, $name:ident, realm_enter) => {
        concat!("int ", stringify!($name), "(uint64_t realm, kiln_realm_entry_fn entry, void *context);\n")
    };
    ($group:ident, $name:ident, realm_entropy_word) => {
        concat!("int ", stringify!($name), "(uint64_t seed, uint64_t *out_word);\n")
    };
    ($group:ident, $name:ident, realm_http_attach) => {
        concat!("int ", stringify!($name), "(uint64_t resource, kiln_realm_http_event_fn run, kiln_realm_drop_fn drop, void *work);\n")
    };
    ($group:ident, $name:ident, realm_http_attach_options) => {
        concat!("int ", stringify!($name), "(uint64_t resource, int allow_half_close, kiln_realm_http_event_fn run, kiln_realm_drop_fn drop, void *work);\n")
    };
    ($group:ident, $name:ident, realm_http_close_mode) => {
        concat!("int ", stringify!($name), "(uint64_t resource, int mode);\n")
    };
    ($group:ident, $name:ident, realm_is_active) => {
        concat!("int ", stringify!($name), "(void);\n")
    };
    ($group:ident, $name:ident, realm_now) => {
        concat!("int ", stringify!($name), "(double *out_milliseconds);\n")
    };
    ($group:ident, $name:ident, realm_now_nanoseconds) => {
        concat!("int ", stringify!($name), "(int64_t *out_high, uint64_t *out_low);\n")
    };
    ($group:ident, $name:ident, realm_queue_work) => {
        concat!("int ", stringify!($name), "(kiln_realm_work_fn run, kiln_realm_drop_fn drop, void *work);\n")
    };
    // The prelude owns the promise job queue and registers it once per realm heap; the realm
    // calls `run` where a promise job belongs in its drain and `pending` to know there is one.
    ($group:ident, $name:ident, realm_set_promise_lane) => {
        concat!("int ", stringify!($name), "(kiln_realm_lane_run_fn run, kiln_realm_lane_pending_fn pending, void *context);\n")
    };
    ($group:ident, $name:ident, realm_read_file) => {
        concat!("int ", stringify!($name), "(uint64_t realm, const uint8_t *path, uint64_t path_len, kiln_realm_completion_fn run, kiln_realm_drop_fn drop, void *work, uint64_t *out_request);\n")
    };
    ($group:ident, $name:ident, realm_set_timer) => {
        concat!("int ", stringify!($name), "(double delay_ms, kiln_realm_work_fn run, kiln_realm_drop_fn drop, void *work, uint64_t *out_id);\n")
    };
    ($group:ident, $name:ident, realm_queue_macrotask) => {
        concat!("int ", stringify!($name), "(kiln_realm_work_fn run, kiln_realm_drop_fn drop, void *work, uint64_t *out_id);\n")
    };
    ($group:ident, $name:ident, realm_start_active_host_op) => {
        concat!("int ", stringify!($name), "(const kiln_async_host_op *operation, kiln_realm_completion_fn run, kiln_realm_drop_fn drop, void *work, uint64_t *out_request);\n")
    };
    ($group:ident, $name:ident, realm_start_host_op) => {
        concat!("int ", stringify!($name), "(uint64_t realm, const kiln_async_host_op *operation, kiln_realm_completion_fn run, kiln_realm_drop_fn drop, void *work, uint64_t *out_request);\n")
    };
    // The field order is the contract; it is spelled once in `kiln_realm_runtime_report`'s doc
    // comment in the provider and read back by `kiln_c99_introspect_dump`. `capacity` in, count
    // out, so a prelude built against a shorter report still understands the prefix it knows.
    ($group:ident, $name:ident, realm_runtime_report) => {
        concat!("int ", stringify!($name), "(uint64_t *out_fields, uint64_t capacity, uint64_t *out_count);\n")
    };
    ($group:ident, $name:ident, realm_symbol_for) => {
        concat!("int ", stringify!($name), "(const kiln_string *key, uint64_t *out_id);\n")
    };
    ($group:ident, $name:ident, realm_symbol_key_for) => {
        concat!("int ", stringify!($name), "(uint64_t id, kiln_string *out_key);\n")
    };
    ($group:ident, $name:ident, realm_value_slot) => {
        concat!("int ", stringify!($name), "(uint32_t key, void **out_slot);\n")
    };
    ($group:ident, $name:ident, node_fs_read_file_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, uint8_t **out_bytes, uint64_t *out_len, int32_t *out_err);\n")
    };
    ($group:ident, $name:ident, node_fs_free_bytes) => {
        concat!("void ", stringify!($name), "(uint8_t *bytes, uint64_t len);\n")
    };
    ($group:ident, $name:ident, node_fs_write_file_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, const uint8_t *bytes, uint64_t len, int32_t *out_err);\n")
    };
    ($group:ident, $name:ident, node_fs_mkdir_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, int32_t recursive, int32_t *out_err);\n")
    };
    ($group:ident, $name:ident, node_fs_stat_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, int32_t follow, uint64_t *out_size, uint32_t *out_flags, double *out_mtime_ms, uint32_t *out_mode, int32_t *out_err);\n")
    };
    ($group:ident, $name:ident, node_fs_readdir_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, uint8_t **out_entries, uint64_t *out_len, uint64_t *out_count, int32_t *out_err);\n")
    };
    ($group:ident, $name:ident, node_fs_realpath_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, uint8_t **out_bytes, uint64_t *out_len, int32_t *out_err);\n")
    };
    ($group:ident, $name:ident, node_fs_unlink_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, int32_t *out_err);\n")
    };
    ($group:ident, $name:ident, node_fs_rmdir_sync) => {
        concat!("int ", stringify!($name), "(const uint8_t *path, uint64_t path_len, int32_t *out_err);\n")
    };
    // Cross-realm ports, workers and shared memory (realms issue 06).
    ($group:ident, $name:ident, worker_post) => {
        concat!("int ", stringify!($name), "(uint32_t op, uint64_t port, uint32_t kind, kiln_worker_token *tokens, uint64_t count);\n")
    };
    ($group:ident, $name:ident, worker_post_plain) => {
        concat!("int ", stringify!($name), "(uint64_t port, uint64_t root, void *context, kiln_worker_plain_describe describe, kiln_worker_plain_field field);\n")
    };
    ($group:ident, $name:ident, worker_take) => {
        concat!("int ", stringify!($name), "(uint64_t port, uint32_t *out_kind, kiln_worker_token **out_tokens, uint64_t *out_count, uint32_t *out_plain, void **out_message);\n")
    };
    ($group:ident, $name:ident, worker_message_free) => {
        concat!("void ", stringify!($name), "(void *message);\n")
    };
    ($group:ident, $name:ident, worker_op) => {
        concat!("int ", stringify!($name), "(uint32_t op, uint64_t a, uint64_t *out);\n")
    };
    ($group:ident, $name:ident, worker_broadcast_join) => {
        concat!("int ", stringify!($name), "(const kiln_string *name, uint64_t *out);\n")
    };
    ($group:ident, $name:ident, worker_spawn) => {
        concat!("int ", stringify!($name), "(const kiln_string *entry, int32_t eval, const kiln_string *argv, uint64_t argc, uint64_t port, uint64_t control, const kiln_string *info, uint64_t *out_key, uint64_t *out_thread);\n")
    };
    ($group:ident, $name:ident, worker_self) => {
        concat!("int ", stringify!($name), "(uint64_t *out_thread, uint64_t *out_port, uint64_t *out_control, const uint8_t **out_info, uint64_t *out_info_len);\n")
    };
    ($group:ident, $name:ident, worker_stdio) => {
        concat!("int ", stringify!($name), "(uint32_t stream, const uint8_t *bytes, uint64_t len);\n")
    };
    ($group:ident, $name:ident, worker_exit) => {
        concat!("int ", stringify!($name), "(int32_t code);\n")
    };
    ($group:ident, $name:ident, shared_create) => {
        concat!("int ", stringify!($name), "(uint64_t length, uint64_t capacity, uint8_t **out_bytes, void **out_handle);\n")
    };
    ($group:ident, $name:ident, shared_release) => {
        concat!("void ", stringify!($name), "(void *handle);\n")
    };
    ($group:ident, $name:ident, shared_grow) => {
        concat!("int ", stringify!($name), "(void *handle, uint64_t length, uint64_t *out);\n")
    };
    ($group:ident, $name:ident, atomics_wait) => {
        concat!("int ", stringify!($name), "(void *handle, uint64_t offset, uint32_t width, int64_t expected, double timeout);\n")
    };
    ($group:ident, $name:ident, atomics_wait_async_register) => {
        concat!("int ", stringify!($name), "(void *handle, uint64_t offset, uint32_t width, int64_t expected, uint64_t *out_ticket);\n")
    };
    ($group:ident, $name:ident, atomics_wait_async_cancel) => {
        concat!("void ", stringify!($name), "(uint64_t ticket);\n")
    };
    ($group:ident, $name:ident, atomics_notify) => {
        concat!("uint64_t ", stringify!($name), "(void *handle, uint64_t offset, uint64_t count);\n")
    };
    // A provider entry point the generated C never calls needs no declaration.
    ($group:ident, $name:ident, $signature:ident) => { "" };
}

macro_rules! provider_http_close_contract_count {
    ($(($group:ident, $name:ident, $signature:ident);)*) => {
        0usize $(+ provider_http_close_contract_count!($group, $name, $signature))*
    };
    (realm, kiln_realm_http_server_close_active, realm_http_close_mode) => { 1usize };
    ($group:ident, $name:ident, $signature:ident) => { 0usize };
}

const _: () = assert!(
    kiln_native_provider_contract::provider_abi_registry!(provider_http_close_contract_count) == 1
);

/// A generated C99 translation unit and its deterministic exported entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CModule {
    /// The exported function name in [`Self::source`].
    pub symbol: String,
    /// Complete, self-contained C99 source for one translation unit.
    pub source: String,
    /// Stable primary FNV-1a identity of the bytecode input and, for [`emit_module`], linkage.
    pub program_hash: u64,
    /// Whether this unit intentionally contains only generic bodies and should favor the
    /// cheapest downstream compilation tier.
    pub generic_only: bool,
    /// Whether this generated unit contains selected generic boxed bodies which are compiled at
    /// O1 while their neighboring generic bodies remain O0.
    pub generic_mixed_optimization: bool,
    /// If non-empty, contains multi-chunk C99 sources for parallel compilation.
    pub chunks: Vec<String>,
    /// Per-chunk value aligned with [`Self::chunks`], identifying chunks with selected generic
    /// bodies. Empty when [`Self::chunks`] is empty.
    pub chunk_generic_mixed_optimization: Vec<bool>,
    /// With [`EmissionPolicy::parallel_workers_unit`], and only when the module has a
    /// qualifying `parallelMap` site: one more translation unit holding the sites' parallel
    /// clones and chunk workers, which [`Self::source`] (or a chunk) declares and calls. It is
    /// to be compiled at the profile's optimising level and linked with the module.
    pub parallel_workers: Option<String>,
    /// C entrypoint source that runs only ModuleDeclarationInstantiation. Native graph launchers
    /// call it for every ESM module before starting evaluation.
    pub instantiation: Option<CModuleUnit>,
}

/// An auxiliary source unit exported with the same native artifact as its evaluation module.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CModuleUnit {
    pub symbol: String,
    /// Helper that allocates and publishes this module's export cells before instantiation.
    pub prepare_symbol: String,
    pub source: String,
    /// Whether the instantiation unit contains selected generic boxed bodies that need O1
    /// compilation while its other generic bodies retain the O0 attribute.
    #[serde(default)]
    pub generic_mixed_optimization: bool,
}

/// Backwards-compatible descriptive alias for callers that call an emitted unit a module.
pub type GeneratedModule = CModule;

/// Why emission was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitError {
    /// The bytecode asks this backend to do something outside its implemented surface.
    Unsupported { feature: String },
    /// The bytecode is malformed and cannot be represented safely as C.
    InvalidProgram { message: String },
}

impl std::fmt::Display for EmitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { feature } => {
                write!(formatter, "unsupported native emission: {feature}")
            }
            Self::InvalidProgram { message } => {
                write!(formatter, "invalid bytecode program: {message}")
            }
        }
    }
}

impl std::error::Error for EmitError {}

/// Emit one scalar bytecode program as a deterministic C99 translation unit.
///
/// The symbol is derived only from the canonical bytecode contents; no source path, timestamp,
/// process id, or caller-provided name appears in the generated output.  This makes equal
/// programs produce byte-for-byte equal objects while still giving each distinct program an
/// externally linkable entry point.  The entry point includes two independently seeded 64-bit
/// FNV-1a digests; [`CModule::program_hash`] remains the primary digest for API compatibility.
pub fn emit(program: &Program) -> Result<CModule, EmitError> {
    emit_with_policy(program, EmissionPolicy::default())
}

pub fn emit_with_policy(program: &Program, policy: EmissionPolicy) -> Result<CModule, EmitError> {
    validate(program)?;
    let (program_hash, symbol_hash) = hash_program_pair_with_policy(program, &policy);
    let symbol = format!("kiln_module_{program_hash:016x}{symbol_hash:016x}");
    let (source, generic_mixed_optimization) = render(program, program_hash, &symbol, &policy)?;
    Ok(CModule {
        symbol,
        source,
        program_hash,
        generic_only: policy.generic_only,
        generic_mixed_optimization,
        chunks: Vec::new(),
        chunk_generic_mixed_optimization: Vec::new(),
        parallel_workers: None,
        instantiation: None,
    })
}

/// Explicitly named spelling for users integrating several backend implementations.
pub fn emit_c(program: &Program) -> Result<CModule, EmitError> {
    emit(program)
}

/// Emit one linkage-aware module as a deterministic C99 translation unit.
///
/// The generated entry point extends the scalar ABI while retaining the original `host` and
/// `result` arguments at the front of the signature:
///
/// ```text
/// int symbol(kiln_host *host, kiln_value *result,
///            const kiln_value *imports, size_t import_count,
///            kiln_value *exports, size_t export_count)
/// ```
///
/// Imports are ordered like [`ModuleProgram::imports`] and exports are ordered like
/// [`ModuleProgram::exports`].  Import slots are initialized before the first instruction and
/// export slots are written only after successful execution.
pub fn emit_module(module: &ModuleProgram) -> Result<CModule, EmitError> {
    emit_module_with_policy(module, EmissionPolicy::default())
}

pub fn emit_module_with_policy(
    module: &ModuleProgram,
    policy: EmissionPolicy,
) -> Result<CModule, EmitError> {
    validate_module(module)?;
    let (program_hash, symbol_hash) = hash_module_pair_with_policy(module, &policy);
    let symbol = format!("kiln_module_{program_hash:016x}{symbol_hash:016x}");
    let rendered = render_module(module, program_hash, &symbol, &policy, false, &symbol)?;
    Ok(CModule {
        symbol,
        source: rendered.source,
        program_hash,
        generic_only: policy.generic_only,
        generic_mixed_optimization: rendered.generic_mixed_optimization,
        chunks: rendered.chunks,
        chunk_generic_mixed_optimization: rendered.chunk_generic_mixed_optimization,
        parallel_workers: rendered.parallel_workers,
        instantiation: None,
    })
}

/// Emit a linkage-aware module with a symbol derived from its normalized logical path.
///
/// Graph launchers need to keep their own object cache entry when a module's body changes.  The
/// ordinary [`emit_module`] symbol intentionally includes the complete bytecode, which is useful
/// for standalone content-addressed units but would make the launcher change for every body edit.
/// A graph caller therefore supplies a project-relative path in canonical slash form.  The path
/// is validated here so the resulting symbol is independent of an absolute checkout location and
/// always has a valid C identifier.
pub fn emit_module_with_logical_path(
    module: &ModuleProgram,
    logical_path: &str,
) -> Result<CModule, EmitError> {
    emit_module_with_logical_path_and_policy(module, logical_path, EmissionPolicy::default())
}

pub fn emit_module_with_logical_path_and_policy(
    module: &ModuleProgram,
    logical_path: &str,
    policy: EmissionPolicy,
) -> Result<CModule, EmitError> {
    emit_module_with_logical_path_and_policy_and_instantiation(module, logical_path, policy, false)
}

/// Emit a graph module, optionally splitting ModuleDeclarationInstantiation from evaluation.
/// The split is used for modules in a cycle; ordinary modules retain their single-entry ABI.
pub fn emit_module_with_logical_path_and_policy_and_instantiation(
    module: &ModuleProgram,
    logical_path: &str,
    policy: EmissionPolicy,
    instantiate: bool,
) -> Result<CModule, EmitError> {
    let (symbol, program_hash, _) =
        module_identity_with_logical_path_and_policy(module, logical_path, &policy)?;
    // A cycle's split entrypoints no longer contain the full module's initialization stores.
    // Preserve only their bounded loop-ranking facts; all call resolution remains phase-local.
    let generic_loop_profile = (instantiate && policy.generic_only)
        .then(|| direct::generic_loop_profile_for_program(&module.program))
        .flatten();
    // The runtime bootstrap reads its captured bindings from the realm's unit block, so that its
    // closures capture nothing and live in the shared immutable region (realms issue 08).
    let region = policy
        .shared_region
        .then(|| module_cells::plan(&module.program))
        .flatten();
    let rewritten;
    let (module, mut region) = match region {
        Some((program, mut plan)) => {
            // A module import/export can pass a cell pointer through the boundary ABI to a
            // different unit. Compact unit-owned cells have no GC block header, so retain the
            // ordinary GC representation whenever such a seam exists. The concatenated
            // runtime bootstrap has neither imports nor exports.
            if !module.imports.is_empty() || !module.exports.is_empty() {
                plan.compact_cells.clear();
            }
            rewritten = ModuleProgram {
                program,
                ..module.clone()
            };
            (&rewritten, Some(std::rc::Rc::new(plan)))
        }
        None => (module, None),
    };
    if instantiate {
        // Export cells are allocated before graph-wide instantiation. Every module-level read or
        // write of an exported binding must therefore go through that preallocated cell, even
        // when no closure in this source happened to require the compiler's ordinary cell flag.
        let mut plan = region
            .take()
            .map(|plan| plan.as_ref().clone())
            .unwrap_or_default();
        plan.cell_globals
            .extend(module.exports.iter().map(|binding| binding.local_slot));
        // Link-time ESM binding imports arrive as cells when the provider is in an instantiated
        // cycle, and as values written into an importer-owned cell otherwise. The module body
        // must dereference either representation before using an imported binding. Namespace
        // imports remain ordinary object values and must not be interpreted as cells.
        plan.cell_globals.extend(
            module
                .imports
                .iter()
                .filter(|binding| binding.imported != kiln_ir::NAMESPACE_IMPORT)
                .map(|binding| binding.local_slot),
        );
        region = Some(std::rc::Rc::new(plan));
    }
    let mut linked_cells = module
        .exports
        .iter()
        .map(|export| export.local_slot)
        .collect::<HashSet<_>>();
    if instantiate {
        linked_cells.extend(
            module
                .imports
                .iter()
                .filter(|binding| binding.imported != kiln_ir::NAMESPACE_IMPORT)
                .map(|binding| binding.local_slot),
        );
        linked_cells.extend(
            module
                .program
                .code
                .iter()
                .take(module.link_facts.module_instantiation_end as usize)
                .filter_map(|instruction| match instruction {
                    Instruction::NewCell(slot) => Some(*slot),
                    _ => None,
                }),
        );
    }
    // Splitting removes hoisted MakeFunction instructions from evaluation. Compute their
    // captured binding roots while the complete module still establishes closure ownership.
    let mut evaluation_policy = policy.clone();
    if instantiate {
        if let Some(demanded) = &policy.live_module_slots {
            let retained = closed_world_function_liveness(
                &module.program,
                demanded,
                module.link_facts.direct_eval,
                &module.link_facts.removable_declarations,
            )
            .2;
            evaluation_policy.live_module_slots = Some(Arc::new(retained));
        }
    }
    let (module, instantiation) = if instantiate {
        let mut module = module.clone();
        let mut binding_slots = module
            .exports
            .iter()
            .map(|binding| binding.local_slot)
            .collect::<HashSet<_>>();
        binding_slots.extend(
            module
                .imports
                .iter()
                .filter(|binding| binding.imported != kiln_ir::NAMESPACE_IMPORT)
                .map(|binding| binding.local_slot),
        );
        // The graph linker installs each binding slot as a cell before this code runs. The
        // ordinary module-body instructions were compiled as locals, though, so convert those
        // accesses to the cell operations the backend already uses for captured bindings.
        // `NewCell` is redundant for these slots and would replace the shared cell.
        for instruction in &mut module.program.code {
            match instruction {
                Instruction::LoadLocal(slot) if binding_slots.contains(slot) => {
                    *instruction = Instruction::LoadCell(*slot);
                }
                Instruction::StoreLocal(slot) if binding_slots.contains(slot) => {
                    *instruction = Instruction::StoreCell(*slot);
                }
                Instruction::NewCell(slot) if binding_slots.contains(slot) => {
                    *instruction = Instruction::Nop;
                }
                _ => {}
            }
        }
        let instantiation_symbol = format!("{symbol}_instantiate");
        let mut evaluation = module.clone();
        for instruction in evaluation
            .program
            .code
            .iter_mut()
            .take(module.link_facts.module_instantiation_end as usize)
        {
            *instruction = Instruction::Nop;
        }
        let mut instantiation = module.clone();
        instantiation
            .program
            .code
            .truncate(module.link_facts.module_instantiation_end as usize);
        instantiation.program.code.push(Instruction::Return);
        // Declaration ranges describe evaluation's original bytecode, not this truncated
        // prefix. In particular the appended Return occupies the first initializer's PC.
        instantiation.link_facts.removable_declarations.clear();
        // An instantiation entrypoint is linked as one auxiliary object. Do not let the normal
        // large-module splitter return chunks that CModuleUnit cannot currently retain.
        let mut instantiation_policy = policy.clone();
        instantiation_policy.compact = false;
        instantiation_policy.parallel_workers_unit = false;
        // Liveness is normally computed independently for each emitted entrypoint. Splitting a
        // cycle module separates the declaration's `MakeFunction` from the evaluation code that
        // calls it, however, so the instantiation pass can otherwise prune a closure that the
        // evaluation pass still loads from its binding cell. Retain only top-level function
        // bindings whose slots are read by the post-instantiation body; this preserves DCE for
        // unrelated declarations and keeps the two phase views consistent.
        if let Some(demanded) = &policy.live_module_slots {
            let boundary = module.link_facts.module_instantiation_end as usize;
            let evaluation_reads = module
                .program
                .code
                .iter()
                .skip(boundary)
                .filter_map(|instruction| match instruction {
                    Instruction::LoadLocal(slot)
                    | Instruction::LoadCell(slot)
                    | Instruction::LoadGlobal(slot) => Some(*slot),
                    _ => None,
                })
                .collect::<HashSet<_>>();
            // Seed phase-one DCE from the same full-module liveness closure that protects the
            // evaluation entrypoint. A hoisted helper can be reachable only through another
            // function which runs after initialization, so scanning only top-level evaluation
            // reads misses it and leaves its exported/imported cell empty.
            let mut retained = evaluation_policy
                .live_module_slots
                .as_ref()
                .map(|slots| (**slots).clone())
                .unwrap_or_else(|| (**demanded).clone());
            for (pc, instruction) in module.program.code[..boundary.min(module.program.code.len())]
                .iter()
                .enumerate()
            {
                let Instruction::MakeFunction(function_index) = instruction else {
                    continue;
                };
                let Some(store) = module.program.code.get(pc + 1) else {
                    continue;
                };
                let slot = match store {
                    Instruction::StoreLocal(slot)
                    | Instruction::StoreCell(slot)
                    | Instruction::StoreGlobal(slot) => *slot,
                    _ => continue,
                };
                // The slot may be used by another module through an exported/imported live
                // binding without appearing in this module's evaluation instructions. `demanded`
                // contains those graph-level roots; their closures and captured locals must be
                // present before any member of the cycle begins evaluation.
                if evaluation_reads.contains(&slot) || demanded.contains(&slot) {
                    retained.insert(slot);
                    // The closure is retained for local evaluation or an imported export, but
                    // module locals its upvalues point at must also survive instantiation DCE.
                    // Otherwise the closure survives while its captured lexical cell is pruned,
                    // so a class or let declaration can leave it permanently in TDZ.
                    if let Some(function) = module.program.functions.get(*function_index as usize) {
                        for upvalue in &function.upvalues {
                            match upvalue {
                                kiln_ir::Upvalue::ParentLocal(captured)
                                | kiln_ir::Upvalue::NamedParentLocal { slot: captured, .. } => {
                                    retained.insert(*captured);
                                }
                                kiln_ir::Upvalue::ParentUpvalue(_)
                                | kiln_ir::Upvalue::NamedParentUpvalue { .. } => {}
                            }
                        }
                    }
                }
            }
            instantiation_policy.live_module_slots = Some(Arc::new(retained));
        }
        let mut source = module_cells::with_plan(region.clone(), || {
            render_module_with_unit(
                &instantiation,
                program_hash,
                &instantiation_symbol,
                Some(&symbol),
                true,
                true,
                &instantiation_policy,
                false,
                logical_path,
                true,
                generic_loop_profile.clone(),
            )
        })?;
        render_prepare_module_cells(
            &mut source.source,
            &format!("{instantiation_symbol}_prepare_cells"),
            &symbol,
            &instantiation,
        );
        (
            evaluation,
            Some(CModuleUnit {
                symbol: instantiation_symbol.clone(),
                prepare_symbol: format!("{instantiation_symbol}_prepare_cells"),
                source: source.source,
                generic_mixed_optimization: source.generic_mixed_optimization,
            }),
        )
    } else {
        (module.clone(), None)
    };
    let rendered = module_cells::with_linked_cells(linked_cells, || {
        module_cells::with_plan(region, || {
            render_module_with_unit(
                &module,
                program_hash,
                &symbol,
                instantiate.then_some(symbol.as_str()),
                instantiate,
                false,
                &evaluation_policy,
                false,
                logical_path,
                instantiate,
                generic_loop_profile,
            )
        })
    })?;
    Ok(CModule {
        symbol,
        source: rendered.source,
        program_hash,
        generic_only: policy.generic_only,
        generic_mixed_optimization: rendered.generic_mixed_optimization,
        chunks: rendered.chunks,
        chunk_generic_mixed_optimization: rendered.chunk_generic_mixed_optimization,
        parallel_workers: rendered.parallel_workers,
        instantiation,
    })
}

fn render_prepare_module_cells(
    output: &mut String,
    prepare_symbol: &str,
    unit_symbol: &str,
    module: &ModuleProgram,
) {
    let imports = module
        .imports
        .iter()
        .map(|binding| binding.local_slot)
        .collect::<HashSet<_>>();
    let tdz = module
        .program
        .code
        .iter()
        .filter_map(|instruction| match instruction {
            Instruction::MarkUninitialized(slot) => Some(*slot),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let slots = module
        .exports
        .iter()
        .filter_map(|binding| {
            let slot = binding.local_slot;
            (!imports.contains(&slot)).then_some((slot, tdz.contains(&slot)))
        })
        .collect::<BTreeMap<_, _>>();
    writeln!(
        output,
        "int {prepare_symbol}(kiln_host *host, kiln_value *cells, size_t count) {{"
    )
    .unwrap();
    writeln!(output, "    if (count != (size_t){} || (count != 0 && cells == NULL)) return KILN_C99_INVALID_ARGUMENT;", module.exports.len()).unwrap();
    writeln!(output, "    if (kiln_realm_is_active()) {{ int status = kiln_realm_gc_register_roots_with_visitor({unit_symbol}_gc_roots, {unit_symbol}_gc_visit_roots, NULL, 0); if (status != KILN_C99_OK) return status; }}").unwrap();
    writeln!(output, "    kiln_val *locals = {unit_symbol}_globals;").unwrap();
    for (slot, uninitialized) in &slots {
        writeln!(
            output,
            "    locals[{slot}] = {};",
            if *uninitialized {
                "KILN_VAL_UNINITIALIZED"
            } else {
                "KILN_VAL_UNDEFINED"
            }
        )
        .unwrap();
        writeln!(output, "    {{ int status = kiln_c99_val_make_cell_owned(&locals[{slot}], host != NULL ? host->module_owner : NULL); if (status != KILN_C99_OK) return status; }}").unwrap();
    }
    for (index, binding) in module.exports.iter().enumerate() {
        if slots.contains_key(&binding.local_slot) {
            writeln!(
                output,
                "    cells[{index}] = kiln_val_to_boundary(locals[{}]);",
                binding.local_slot
            )
            .unwrap();
        } else {
            writeln!(output, "    cells[{index}] = kiln_c99_undefined();").unwrap();
        }
    }
    output.push_str("    return KILN_C99_OK;\n}\n");
}

/// Compute the stable symbol and both program hashes without rendering the C body.
///
/// Native orchestration uses this to look up a generated source artifact before paying for SSA
/// planning and rendering. Validation remains part of the contract so a cache hit cannot make an
/// invalid module acceptable.
pub fn module_identity_with_logical_path_and_policy(
    module: &ModuleProgram,
    logical_path: &str,
    policy: &EmissionPolicy,
) -> Result<(String, u64, u64), EmitError> {
    validate_logical_path(logical_path)?;
    validate_module(module)?;
    let (program_hash, seeded_program_hash) = hash_module_pair_with_policy(module, policy);
    let (path_hash, path_hash_seeded) = hash_logical_path_pair(logical_path);
    Ok((
        format!("kiln_module_path_{path_hash:016x}{path_hash_seeded:016x}"),
        program_hash,
        seeded_program_hash,
    ))
}

/// Emit a CommonJS body whose private graph ABI exposes the final `module.exports` value as one
/// synthetic export slot. The five wrapper imports remain ordinary module imports, so compiled
/// closures capture the exact `exports`, `require`, `module`, `__filename`, and `__dirname`
/// identities supplied by the graph launcher. This does not widen the public scalar embedding
/// ABI; it is used only between native graph objects.
pub fn emit_commonjs_module_with_logical_path_and_policy(
    module: &ModuleProgram,
    logical_path: &str,
    policy: EmissionPolicy,
) -> Result<CModule, EmitError> {
    validate_logical_path(logical_path)?;
    validate_module(module)?;
    if module.imports.len() < 3 || module.imports[2].imported != "module" {
        return Err(EmitError::InvalidProgram {
            message: "CommonJS module is missing its canonical wrapper imports".to_owned(),
        });
    }
    let (program_hash, _) = hash_module_pair_with_policy(module, &policy);
    let (path_hash, path_hash_seeded) = hash_logical_path_pair(logical_path);
    let symbol = format!("kiln_module_path_{path_hash:016x}{path_hash_seeded:016x}");
    let rendered = render_module(module, program_hash, &symbol, &policy, true, logical_path)?;
    Ok(CModule {
        symbol,
        source: rendered.source,
        program_hash,
        generic_only: policy.generic_only,
        generic_mixed_optimization: rendered.generic_mixed_optimization,
        chunks: rendered.chunks,
        chunk_generic_mixed_optimization: rendered.chunk_generic_mixed_optimization,
        parallel_workers: rendered.parallel_workers,
        instantiation: None,
    })
}

/// Emit one typed runtime-provided module inside the backend's own value representation.
///
/// `body` runs after the ordinary module ABI validation and receives the canonical `exports`
/// array. It may use private `kiln_c99_*` helpers because it is appended inside the same
/// translation unit; callers never construct or inspect a `kiln_value`. This is the narrow seam
/// used by virtual modules whose values cannot be represented by bytecode alone.
pub fn emit_runtime_module_with_logical_path(
    module: &ModuleProgram,
    logical_path: &str,
    body: &str,
) -> Result<CModule, EmitError> {
    emit_runtime_module_with_logical_path_and_policy(
        module,
        logical_path,
        body,
        EmissionPolicy::default(),
    )
}

/// Emit a runtime-provided module under the same prelude policy as ordinary graph modules.
pub fn emit_runtime_module_with_logical_path_and_policy(
    module: &ModuleProgram,
    logical_path: &str,
    body: &str,
    policy: EmissionPolicy,
) -> Result<CModule, EmitError> {
    validate_logical_path(logical_path)?;
    validate_module(module)?;
    if body.as_bytes().contains(&0) {
        return Err(EmitError::InvalidProgram {
            message: "runtime module body contains an embedded NUL".to_owned(),
        });
    }
    let (program_hash, _) = hash_module_pair_with_policy(module, &policy);
    let (path_hash, path_hash_seeded) = hash_logical_path_pair(logical_path);
    let symbol = format!("kiln_module_path_{path_hash:016x}{path_hash_seeded:016x}");
    let source = render_runtime_module(module, program_hash, &symbol, body, &policy);
    Ok(CModule {
        symbol,
        source,
        program_hash,
        generic_only: policy.generic_only,
        generic_mixed_optimization: false,
        chunks: Vec::new(),
        chunk_generic_mixed_optimization: Vec::new(),
        parallel_workers: None,
        instantiation: None,
    })
}

fn validate_logical_path(logical_path: &str) -> Result<(), EmitError> {
    let invalid = |message: &str| {
        Err(EmitError::InvalidProgram {
            message: format!("invalid logical module path: {message}"),
        })
    };
    if logical_path.is_empty() {
        return invalid("path is empty");
    }
    if logical_path.as_bytes().contains(&0) {
        return invalid("path contains an embedded NUL");
    }
    // Logical paths are deliberately stricter than host filesystem paths.  In particular, a
    // leading slash, backslash, or drive prefix could smuggle an absolute checkout path into the
    // supposedly project-relative identity, and empty/dot components are not normalized input.
    if logical_path.starts_with('/')
        || logical_path.starts_with('\\')
        || logical_path.as_bytes().get(1) == Some(&b':')
        || logical_path.contains('\\')
    {
        return invalid("path is not project-relative slash-separated input");
    }
    if logical_path
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return invalid("path contains an empty, `.` or `..` component");
    }
    Ok(())
}

mod prelude;
pub use prelude::*;

mod validation;
pub(crate) use validation::*;

mod direct;
pub mod emit_graph;
pub mod facts;
pub mod graph;
mod module_cells;
pub mod parallel;
pub mod parallel_heap;
pub mod regex;
pub mod state_owners;
pub mod summary;
mod tail;
pub mod verifier;
pub use emit_graph::emit_function_from_graph;
pub use facts::{
    FunctionFacts, ModuleFactsReport, ProgramFacts, SiteFact, analyze_graph, analyze_program,
};
pub use graph::{
    BasicBlock, BlockId, FunctionGraph, FunctionTypeProfile, GraphOp, SiteId, SiteTypeProfile,
    Terminator, TypeProfile, ValueId, build_function_graph,
};
pub use kiln_db::summaries_digest;
pub use summary::{
    FunctionSummary, TypeGuess, summarize_function, summarize_graph, summarize_program,
};
pub use verifier::{VerifierError, verify_function_graph};

/// Whether any body in this program can produce a value the embedding boundary cannot carry.
///
/// The boundary conversions depend on it, and they are not free to emit unconditionally:
/// reaching `kiln_c99_text` from every entry point makes every unit reference the number
/// provider, which the graph link tests deliberately link without. It would also change the
/// bytes of a scalar unit, and those bytes are a cache key.
///
/// So the question is asked the other way round: a program whose every instruction is in the
/// original scalar set can only ever hold a scalar, and everything else is assumed to be able to
/// produce a reference. Listing what *cannot* rather than what can is what makes this safe to
/// extend -- a new instruction is covered the day it is added, where an allowlist would have
/// silently let its values cross unconverted. That is exactly what happened: the list named
/// `MakeFunction` and `LoadBuiltin`, so `console.log([1])` -- a program with neither -- handed
/// the host a raw array and failed with no diagnostic.
fn has_top_level_await(program: &Program) -> bool {
    program.code.iter().any(|instruction| {
        matches!(
            instruction,
            Instruction::Await | Instruction::Yield | Instruction::YieldStar(_)
        )
    })
}

fn produces_references(program: &Program) -> bool {
    if has_top_level_await(program) {
        return true;
    }
    // A cell read can be a temporal-dead-zone read. The scalar entry has no `thrown` slot,
    // while the boxed entry must preserve the actual ReferenceError for the host (and for JS
    // catch blocks). Do not classify a program containing one as a non-throwing scalar body.
    let reads_shared_cell = |code: &[Instruction]| {
        code.iter().any(|instruction| {
            matches!(instruction, Instruction::LoadGlobal(slot) if module_cells::is_cell_global(*slot))
        })
    };
    if reads_shared_cell(&program.code)
        || program
            .functions
            .iter()
            .any(|proto| reads_shared_cell(&proto.code))
    {
        return true;
    }
    let scalar_only = |code: &[Instruction]| {
        code.iter().all(|instruction| {
            matches!(
                instruction,
                Instruction::Const(_)
                    | Instruction::LoadLocal(_)
                    | Instruction::StoreLocal(_)
                    | Instruction::LoadGlobal(_)
                    | Instruction::StoreGlobal(_)
                    | Instruction::NewCell(_)
                    | Instruction::StoreCell(_)
                    | Instruction::Pop
                    | Instruction::Dup
                    | Instruction::Unary(_)
                    | Instruction::Binary(_)
                    | Instruction::Jump(_)
                    | Instruction::JumpIfFalse(_)
                    | Instruction::JumpIfTruthy(_)
                    | Instruction::JumpIfNotNullish(_)
                    | Instruction::JumpIfReturnSignal(_)
                    | Instruction::HostCall { .. }
                    | Instruction::Return
            )
        })
    };
    // A BigInt is the exception to the list above: it makes an *operator* throw -- `1n + 1` is
    // a TypeError and `1n / 0n` a RangeError -- so a body holding one needs somewhere to put the
    // thrown value even when every instruction in it is scalar.
    program
        .constants
        .iter()
        .any(|constant| matches!(constant, Constant::BigInt(_)))
        || !scalar_only(&program.code)
        || !program
            .functions
            .iter()
            .all(|proto| scalar_only(&proto.code))
}

/// Whether this function proto produces references or needs checked arithmetic.
/// False for a purely scalar function that holds no BigInt and cannot throw or refuse.
fn proto_produces_references(proto: &FunctionProto, program: &Program) -> bool {
    if proto.generator
        || proto.asynchronous
        || proto.mapped_arguments
        || proto.parameter_count > 0
        || !proto.upvalues.is_empty()
    {
        return true;
    }
    let scalar_only = |code: &[Instruction]| {
        code.iter().all(|instruction| match instruction {
            Instruction::Const(c_idx) => matches!(
                program.constants.get(*c_idx as usize),
                Some(Constant::Number(_)) | Some(Constant::Boolean(_)) | Some(Constant::Null)
            ),
            Instruction::LoadLocal(_)
            | Instruction::StoreLocal(_)
            | Instruction::Pop
            | Instruction::Dup
            | Instruction::Dup2
            | Instruction::Unary(_)
            | Instruction::Jump(_)
            | Instruction::JumpIfFalse(_)
            | Instruction::JumpIfTruthy(_)
            | Instruction::JumpIfNotNullish(_)
            | Instruction::JumpIfReturnSignal(_)
            | Instruction::Return
            | Instruction::MarkUninitialized(_) => true,
            Instruction::Binary(op) => !matches!(op, BinaryOp::In | BinaryOp::InstanceOf),
            _ => false,
        })
    };
    !scalar_only(&proto.code)
}

/// Whether anything in this program reaches module scope from inside a call.
///
/// A module binding outlives every call and is shared by all of them, so when one is read from a
/// function body it cannot live in the entry point's automatic frame -- that frame is gone by
/// the time an exported closure runs. It becomes a file-scope array instead, which is what a
/// per-module singleton is.
fn uses_module_scope(program: &Program) -> bool {
    let reaches = |code: &[Instruction]| {
        code.iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::LoadGlobal(_) | Instruction::StoreGlobal(_)
            )
        })
    };
    has_top_level_await(program)
        || reaches(&program.code)
        || program.functions.iter().any(|proto| reaches(&proto.code))
}

/// Local slots this body treats as cells without ever creating one.
///
/// For an import, whose cell belongs to the exporter and arrives through the linker. The
/// compiler always lowers an import to `LoadCell`, because an import is a live view of the
/// exporter's binding -- but hand-written bytecode need not, and a body that treats the slot as
/// a plain local has to keep getting a plain local. So this asks the body rather than assuming.
fn slots_read_as_cells(code: &[Instruction], functions: &[FunctionProto]) -> HashSet<u32> {
    let mut needed = HashSet::new();
    let mut created = HashSet::new();
    for instruction in code {
        match instruction {
            Instruction::LoadCell(slot) | Instruction::StoreCell(slot) => {
                needed.insert(*slot);
            }
            Instruction::NewCell(slot) => {
                created.insert(*slot);
            }
            // Capturing a slot is only possible if it holds a cell.
            Instruction::MakeFunction(proto) => {
                if let Some(target) = functions.get(*proto as usize) {
                    for upvalue in &target.upvalues {
                        if let Upvalue::ParentLocal(slot) | Upvalue::NamedParentLocal { slot, .. } =
                            upvalue
                        {
                            needed.insert(*slot);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    needed.retain(|slot| !created.contains(slot));
    needed
}

/// Emit one C function per compiled body, forward-declared so a closure made before a body is
/// rendered can still point at it.
/// How a builtin reaches the program: a value the runtime hands out by name, or an object the
/// runtime has to build, which can fail. One table, both emitters.
#[derive(Debug, Clone, Copy)]
pub(crate) enum BuiltinLoad {
    /// A C expression of type `kiln_value`.
    Value(&'static str),
    /// A C function `int f(kiln_value *out)`.
    Build(&'static str),
    /// A global the runtime bootstrap defines in TypeScript (ADR 0005, ADR 0063 rule 4), read
    /// by name from the global object; a refusal names it if the bootstrap has not run.
    Global(&'static str),
}

pub(crate) fn builtin_load(builtin: kiln_ir::Builtin) -> BuiltinLoad {
    match builtin {
        // Built rather than named: real objects, because a program writes to them.
        kiln_ir::Builtin::GlobalThis => BuiltinLoad::Build("kiln_c99_global_object"),
        kiln_ir::Builtin::Process => BuiltinLoad::Build("kiln_c99_process_object"),
        kiln_ir::Builtin::Atomics => BuiltinLoad::Build("kiln_c99_atomics_object"),
        kiln_ir::Builtin::SetImmediate => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_SET_IMMEDIATE)")
        }
        kiln_ir::Builtin::ClearImmediate => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_CLEAR_IMMEDIATE)")
        }
        // `NaN` and `Infinity` are values rather than callables, but they reach the program
        // the same way -- as a name the compiler resolves to something the runtime supplies.
        kiln_ir::Builtin::String => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_STRING)"),
        kiln_ir::Builtin::Math => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_MATH)"),
        kiln_ir::Builtin::Number => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_NUMBER)"),
        kiln_ir::Builtin::Object => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT)"),
        kiln_ir::Builtin::Array => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY)"),
        kiln_ir::Builtin::Json => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_JSON)"),
        kiln_ir::Builtin::Date => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_DATE)"),
        kiln_ir::Builtin::Map => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_MAP)"),
        kiln_ir::Builtin::Set => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_SET)"),
        kiln_ir::Builtin::WeakMap => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_WEAK_MAP)")
        }
        kiln_ir::Builtin::WeakSet => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_WEAK_SET)")
        }
        kiln_ir::Builtin::Promise => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_PROMISE)")
        }
        kiln_ir::Builtin::Error => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_ERROR)"),
        kiln_ir::Builtin::TypeError => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_TYPE_ERROR)")
        }
        kiln_ir::Builtin::RangeError => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_RANGE_ERROR)")
        }
        kiln_ir::Builtin::ReferenceError => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_REFERENCE_ERROR)")
        }
        kiln_ir::Builtin::SyntaxError => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_SYNTAX_ERROR)")
        }
        kiln_ir::Builtin::UriError => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_URI_ERROR)")
        }
        kiln_ir::Builtin::EvalError => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_EVAL_ERROR)")
        }
        // Reading `eval` as a value (for example through `%eval%` in get-intrinsic) is supported.
        // Direct calls carry a `DirectEval` instruction and remain separately guarded by
        // validation until the native backend can compile them with the caller's lexical cells.
        kiln_ir::Builtin::Eval => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_EVAL)"),
        kiln_ir::Builtin::AggregateError => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_AGGREGATE_ERROR)")
        }
        kiln_ir::Builtin::SuppressedError => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_SUPPRESSED_ERROR)")
        }
        kiln_ir::Builtin::DisposableStack => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_DISPOSABLE_STACK)")
        }
        kiln_ir::Builtin::AsyncDisposableStack => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_ASYNC_DISPOSABLE_STACK)")
        }
        kiln_ir::Builtin::Boolean => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_BOOLEAN)")
        }
        kiln_ir::Builtin::ParseInt => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_PARSE_INT)")
        }
        kiln_ir::Builtin::ParseFloat => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_PARSE_FLOAT)")
        }
        kiln_ir::Builtin::IsNaN => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_GLOBAL_IS_NAN)")
        }
        kiln_ir::Builtin::IsFinite => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_GLOBAL_IS_FINITE)")
        }
        kiln_ir::Builtin::Reflect => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_REFLECT)")
        }
        kiln_ir::Builtin::Proxy => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_PROXY)"),
        kiln_ir::Builtin::Symbol => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_SYMBOL)"),
        kiln_ir::Builtin::RegExp => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_REGEXP)"),
        kiln_ir::Builtin::Function => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION)")
        }
        kiln_ir::Builtin::BigInt => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_BIGINT)"),
        kiln_ir::Builtin::SetTimeout => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_SET_TIMEOUT)")
        }
        kiln_ir::Builtin::SetInterval => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_SET_INTERVAL)")
        }
        kiln_ir::Builtin::ClearTimeout => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_CLEAR_TIMEOUT)")
        }
        kiln_ir::Builtin::ClearInterval => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_CLEAR_INTERVAL)")
        }
        kiln_ir::Builtin::QueueMicrotask => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_QUEUE_MICROTASK)")
        }
        kiln_ir::Builtin::Escape => BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_ESCAPE)"),
        kiln_ir::Builtin::Unescape => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_UNESCAPE)")
        }
        kiln_ir::Builtin::EncodeUri => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_ENCODE_URI)")
        }
        kiln_ir::Builtin::EncodeUriComponent => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_ENCODE_URI_COMPONENT)")
        }
        kiln_ir::Builtin::DecodeUri => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_DECODE_URI)")
        }
        kiln_ir::Builtin::DecodeUriComponent => {
            BuiltinLoad::Value("kiln_c99_method(KILN_C99_METHOD_DECODE_URI_COMPONENT)")
        }
        kiln_ir::Builtin::ArrayBuffer => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER)")
        }
        kiln_ir::Builtin::Int8Array => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_INT8_ARRAY)")
        }
        kiln_ir::Builtin::Uint8Array => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_UINT8_ARRAY)")
        }
        kiln_ir::Builtin::Uint8ClampedArray => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY)")
        }
        kiln_ir::Builtin::Int16Array => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_INT16_ARRAY)")
        }
        kiln_ir::Builtin::Uint16Array => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_UINT16_ARRAY)")
        }
        kiln_ir::Builtin::Int32Array => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_INT32_ARRAY)")
        }
        kiln_ir::Builtin::Uint32Array => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_UINT32_ARRAY)")
        }
        kiln_ir::Builtin::Float32Array => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_FLOAT32_ARRAY)")
        }
        kiln_ir::Builtin::Float64Array => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_FLOAT64_ARRAY)")
        }
        // The class the runtime bootstrap defined (ADR 0063 rule 4), read back from the realm
        // slot `__kilnUrlAdopt` filled; `KILN_C99_BUILTIN_URL` is no longer a value a program sees.
        kiln_ir::Builtin::Url => BuiltinLoad::Build("kiln_c99_url_class"),
        kiln_ir::Builtin::Buffer => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_BUFFER)"),
        kiln_ir::Builtin::NotANumber => BuiltinLoad::Value("kiln_c99_number(0.0 / 0.0)"),
        kiln_ir::Builtin::Infinity => BuiltinLoad::Value("kiln_c99_number(1.0 / 0.0)"),
        // The public WebIDL class is installed by the shared TypeScript bootstrap; the
        // primitive controller remains reachable only through its captured private kernel.
        kiln_ir::Builtin::AbortController => BuiltinLoad::Global("AbortController"),
        kiln_ir::Builtin::AbortSignal => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_ABORT_SIGNAL)")
        }
        kiln_ir::Builtin::TextEncoder => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_TEXT_ENCODER)")
        }
        kiln_ir::Builtin::TextDecoder => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_TEXT_DECODER)")
        }
        kiln_ir::Builtin::Performance => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_PERFORMANCE)")
        }
        kiln_ir::Builtin::StructuredClone => BuiltinLoad::Global("structuredClone"),
        #[allow(unreachable_patterns)]
        _ => unreachable!("verification rejects every other builtin before emission"),
    }
}

/// What a closure carries, as the C expressions `kiln_c99_make_closure` and
/// `kiln_c99_region_closure` take, decided once for both.
struct ClosureFields {
    name: String,
    source: String,
    source_len: usize,
    diagnostic_source: String,
    arity: u32,
    generator: i32,
    asynchronous: i32,
    async_generator: i32,
    function_kind: i32,
    arrow: i32,
    strict: i32,
    frame_locals: u32,
    frame_stack: usize,
    frame_handlers: usize,
    prologue_resume: usize,
}

#[allow(clippy::too_many_arguments)]
fn closure_fields(
    functions: &[FunctionProto],
    source: &str,
    frame_stacks: &[usize],
    strict_functions: &[bool],
    symbol: &str,
    diagnostic_site: &str,
    proto: u32,
    publishes_frame: bool,
) -> ClosureFields {
    let target = &functions[proto as usize];
    let name = target.effective_name_units().map_or_else(
        || "NULL".to_owned(),
        |_| format!("&{symbol}_function_name_{proto}"),
    );
    let span = target.span;
    let (source, source_len) =
        if source.is_empty() || span.end as usize > source.len() || span.start > span.end {
            ("NULL".to_owned(), 0usize)
        } else {
            (
                format!("{symbol}_source + {}", span.start),
                (span.end - span.start) as usize,
            )
        };
    // An arrow is `1` when it reads the enclosing frame (its own `arguments`/`new.target`, or a
    // nested arrow's) and `2` when it provably never does: `kiln_c99_make_closure` then skips
    // materialising the enclosing call's `arguments` object for it.
    let arrow = match (target.arrow, publishes_frame) {
        (false, _) => 0,
        (true, true) => 1,
        (true, false) => 2,
    };
    ClosureFields {
        name,
        source,
        source_len,
        diagnostic_source: diagnostic_source(diagnostic_site),
        arity: target.declared_length,
        generator: i32::from(target.generator || target.asynchronous),
        asynchronous: i32::from(target.asynchronous),
        async_generator: i32::from(target.async_generator),
        function_kind: if target.is_class {
            2
        } else {
            i32::from(target.is_method)
        },
        arrow,
        strict: i32::from(
            strict_functions
                .get(proto as usize)
                .copied()
                .unwrap_or(true),
        ),
        frame_locals: target.local_count.max(1),
        frame_stack: frame_stacks
            .get(proto as usize)
            .copied()
            .unwrap_or(1)
            .max(1),
        frame_handlers: target.code.len().max(1),
        prologue_resume: if binds_at_call(target) {
            target.code.len()
        } else {
            0
        },
    }
}

/// The one call that makes a closure, for both emitters.
///
/// Everything a closure carries is decided here, once: its name and source slice for
/// `Function.prototype.toString`, its kind, its strictness (which decides the receiver a bare
/// call gets), its `length`, the frame a generator needs, the seam where parameter binding
/// stops, and the receiver an arrow snapshots. `captured` is the C expression for the upvalue
/// array, or `NULL`. The result is the C expression that fills `built`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn closure_construction(
    functions: &[FunctionProto],
    source: &str,
    frame_stacks: &[usize],
    strict_functions: &[bool],
    symbol: &str,
    diagnostic_site: &str,
    proto: u32,
    defining_receiver: &str,
    captured: &str,
    capture_count: usize,
    publishes_frame: bool,
) -> String {
    let ClosureFields {
        name,
        source,
        source_len,
        diagnostic_source,
        arity,
        generator,
        asynchronous,
        async_generator,
        function_kind,
        arrow,
        strict,
        frame_locals,
        frame_stack,
        frame_handlers,
        prologue_resume,
    } = closure_fields(
        functions,
        source,
        frame_stacks,
        strict_functions,
        symbol,
        diagnostic_site,
        proto,
        publishes_frame,
    );
    format!(
        "kiln_c99_make_closure(&built, {symbol}_fn_{proto}, {name}, {source}, {source_len}, {diagnostic_source}, UINT32_C({proto}), UINT32_C({arity}), {generator}, {asynchronous}, {async_generator}, {function_kind}, {arrow}, {strict}, {defining_receiver}, kiln_val_from_boundary(kiln_c99_new_target), UINT32_C({frame_locals}), UINT32_C({frame_stack}), UINT32_C({frame_handlers}), UINT32_C({prologue_resume}), {captured}, UINT32_C({capture_count}))"
    )
}

/// The call that makes a closure at a region site (realms issue 08;
/// `module_cells`): the same fields as [`closure_construction`], and the
/// site's process-wide slot `{symbol}_region_sites[site]`. The closure captures nothing, so there
/// is no upvalue array; whether it publishes its frame is passed in, since a region closure is
/// never written after it is made.
#[allow(clippy::too_many_arguments)]
pub(crate) fn region_closure_construction(
    functions: &[FunctionProto],
    source: &str,
    frame_stacks: &[usize],
    strict_functions: &[bool],
    symbol: &str,
    diagnostic_site: &str,
    proto: u32,
    defining_receiver: &str,
    site: usize,
    publishes_frame: bool,
) -> String {
    let ClosureFields {
        name,
        source,
        source_len,
        diagnostic_source,
        arity,
        generator,
        asynchronous,
        async_generator,
        function_kind,
        arrow,
        strict,
        frame_locals,
        frame_stack,
        frame_handlers,
        prologue_resume,
    } = closure_fields(
        functions,
        source,
        frame_stacks,
        strict_functions,
        symbol,
        diagnostic_site,
        proto,
        publishes_frame,
    );
    let publishes = i32::from(publishes_frame);
    format!(
        "kiln_c99_region_closure(&built, &{symbol}_region_sites[{site}], {symbol}_fn_{proto}, {name}, {source}, {source_len}, {diagnostic_source}, UINT32_C({proto}), UINT32_C({arity}), {generator}, {asynchronous}, {async_generator}, {function_kind}, {arrow}, {strict}, {defining_receiver}, UINT32_C({frame_locals}), UINT32_C({frame_stack}), UINT32_C({frame_handlers}), UINT32_C({prologue_resume}), {publishes})"
    )
}

/// Pack the source ranges observable through live closures and rewrite their spans into the pack.
///
/// A bundled module's original source commonly contains large top-level tables and functions that
/// closed-world emission removed. Keeping all of it as one escaped C string makes the C compiler
/// parse dead JavaScript. Ranges are unioned before copying because nested closures overlap their
/// parent's source; copying once per function would grow precisely the modules this is for.
fn compact_closure_source(
    program: &mut Program,
    reachable: Option<&HashSet<usize>>,
    omit_source: bool,
) {
    if omit_source {
        program.source = String::new().into();
        for function in &mut program.functions {
            function.span.start = 1;
            function.span.end = 0;
        }
        return;
    }
    if program.source.is_empty() || program.functions.is_empty() {
        return;
    }
    let original = std::mem::take(&mut program.source);
    let mut intervals = program
        .functions
        .iter()
        .enumerate()
        .filter(|(index, _)| reachable.is_none_or(|live| live.contains(index)))
        .filter_map(|(_, function)| {
            let start = function.span.start as usize;
            let end = function.span.end as usize;
            (start <= end && original.get(start..end).is_some()).then_some((start, end))
        })
        .collect::<Vec<_>>();
    intervals.sort_unstable();
    let mut merged = Vec::<(usize, usize)>::with_capacity(intervals.len());
    for (start, end) in intervals {
        match merged.last_mut() {
            Some((_, previous_end)) if start <= *previous_end => {
                *previous_end = (*previous_end).max(end);
            }
            _ => merged.push((start, end)),
        }
    }

    let mut packed =
        String::with_capacity(merged.iter().map(|(start, end)| end - start).sum::<usize>());
    let mut packed_intervals = Vec::with_capacity(merged.len());
    for (start, end) in merged {
        let packed_start = packed.len();
        packed.push_str(&original[start..end]);
        packed_intervals.push((start, end, packed_start));
    }
    let mut span_map = HashMap::new();
    for (index, function) in program.functions.iter_mut().enumerate() {
        let start = function.span.start as usize;
        let end = function.span.end as usize;
        let mapped = reachable
            .is_none_or(|live| live.contains(&index))
            .then(|| {
                packed_intervals.iter().find_map(|&(from, to, at)| {
                    (from <= start && end <= to).then_some((at + start - from, at + end - from))
                })
            })
            .flatten();
        if let Some((start, end)) = mapped {
            span_map.insert(
                function.span,
                kiln_ir::Span {
                    start: start as u32,
                    end: end as u32,
                },
            );
            function.span.start = start as u32;
            function.span.end = end as u32;
        } else {
            // Preserve the old invalid-span answer (`NULL`, length zero) against the new source.
            function.span.start = 1;
            function.span.end = 0;
        }
    }
    for site in &mut program.debug_call_sites {
        if let Some(original) = site.function_span
            && let Some(compacted) = span_map.get(&original)
        {
            site.function_span = Some(*compacted);
        }
    }
    program
        .debug_call_sites
        .sort_by_key(|site| (site.function_span, site.instruction));
    program.source = packed.into();
}

struct RenderedFunctions {
    sites: usize,
    function_declarations: Vec<String>,
}

struct RenderedModule {
    source: String,
    chunks: Vec<String>,
    generic_mixed_optimization: bool,
    chunk_generic_mixed_optimization: Vec<bool>,
    parallel_workers: Option<String>,
}

fn has_closure_instruction(program: &Program) -> bool {
    program
        .code
        .iter()
        .any(|i| matches!(i, Instruction::MakeFunction(_)))
        || program.functions.iter().any(|f| {
            f.code
                .iter()
                .any(|i| matches!(i, Instruction::MakeFunction(_)))
        })
}

#[allow(clippy::too_many_arguments)]
fn render_functions(
    output: &mut String,
    program: &Program,
    plan: &StackPlan,
    symbol: &str,
    diagnostic_site: &str,
    strict_functions: &[bool],
    graphs: &[FunctionGraph],
    direct: &mut direct::DirectPlan,
    policy: &EmissionPolicy,
    reachable: Option<&HashSet<usize>>,
    should_chunk: bool,
    mut chunked_bodies: Option<&mut Vec<String>>,
    mut workers: Option<(&str, &mut String)>,
) -> RenderedFunctions {
    let mut fn_declarations: Vec<String> = vec![String::new(); program.functions.len()];
    if policy.type_profile_record {
        output.push_str(
            "static inline uint32_t kiln_profile_tag_of(kiln_val v) {\n\
             if (KILN_VAL_IS_DOUBLE(v)) return 1;\n\
             if (KILN_VAL_IS_BOOLEAN(v)) return 2;\n\
             if (KILN_VAL_IS_STRING(v)) return 4;\n\
             if (KILN_VAL_IS_OBJECT(v)) return 8;\n\
             if (KILN_VAL_IS_UNDEFINED(v)) return 16;\n\
             if (KILN_VAL_IS_NULL(v)) return 32;\n\
             return 64;\n\
             }\n\
             static inline const char *kiln_profile_tag_name(uint32_t tag) {\n\
             switch (tag) {\n\
             case 1: return \"number\";\n\
             case 2: return \"boolean\";\n\
             case 4: return \"string\";\n\
             case 8: return \"object\";\n\
             case 16: return \"undefined\";\n\
             case 32: return \"null\";\n\
             default: return \"unknown\";\n\
             }\n\
             }\n",
        );
        writeln!(output, "static void {symbol}_dump_type_profile(void);").unwrap();
        for (index, proto) in program.functions.iter().enumerate() {
            if proto.parameter_count > 0 {
                writeln!(
                    output,
                    "static uint32_t {symbol}_fn_{index}_param_tags[{}] = {{0}};",
                    proto.parameter_count
                )
                .unwrap();
            }
            writeln!(
                output,
                "static uint32_t {symbol}_fn_{index}_return_tag = 0;"
            )
            .unwrap();
        }
    }
    let receiver_columns: Vec<usize> = if policy.generic_only {
        vec![0; graphs.len()]
    } else {
        graphs.iter().map(emit_graph::this_site_count).collect()
    };
    direct.set_receiver_columns(&receiver_columns);
    // A typed entry over a restartable body is emitted as one: decided by emitting it, which
    // depends on the plan and not on what the entry turns out to need.
    let typed_restartable: Vec<bool> = program
        .functions
        .iter()
        .enumerate()
        .map(|(index, proto)| {
            direct.typed[index].as_ref().is_some_and(|typed| {
                emit_graph::emit_restartable(
                    &graphs[index],
                    proto,
                    program,
                    symbol,
                    index,
                    !proto_produces_references(proto, program),
                    diagnostic_site,
                    &plan.functions,
                    strict_functions,
                    typed,
                    emit_graph::EntryKind::Typed {
                        returns_number: typed.typed_returns[index],
                        restartable: true,
                    },
                    false,
                )
                .is_some()
            })
        })
        .collect();
    let emit_typed =
        |output: &mut String, index: usize, typed: &direct::CallPlan, profiled: bool| {
            let proto = &program.functions[index];
            let kind = emit_graph::EntryKind::Typed {
                returns_number: typed.typed_returns[index],
                restartable: typed_restartable[index],
            };
            if typed_restartable[index] {
                let text = emit_graph::emit_restartable(
                    &graphs[index],
                    proto,
                    program,
                    symbol,
                    index,
                    !proto_produces_references(proto, program),
                    diagnostic_site,
                    &plan.functions,
                    strict_functions,
                    typed,
                    kind,
                    profiled && policy.type_profile_record,
                )
                .expect("a restartable body stays restartable");
                output.push_str(&text);
            } else {
                let func_profile = if profiled {
                    policy.type_profile.as_ref().and_then(|p| p.function(index))
                } else {
                    None
                };
                emit_graph::emit_function_from_graph_with_profile(
                    output,
                    &graphs[index],
                    proto,
                    program,
                    symbol,
                    index,
                    !proto_produces_references(proto, program),
                    diagnostic_site,
                    &plan.functions,
                    strict_functions,
                    typed,
                    kind,
                    func_profile,
                    profiled && policy.type_profile_record,
                    0,
                );
            }
        };
    // What each typed entry needs is what its emitted body names, and what its typed callees
    // need. Emit the typed bodies to find out, until nothing more is needed.
    let mut needs = vec![direct::TypedNeeds::default(); program.functions.len()];
    // Issue 20: what each typed body speculates on with no way back (its own guards), and which
    // typed entries it calls, read from the same text.
    let mut relies: Vec<std::collections::BTreeSet<String>> =
        vec![std::collections::BTreeSet::new(); program.functions.len()];
    let mut typed_callees: Vec<std::collections::BTreeSet<usize>> =
        vec![std::collections::BTreeSet::new(); program.functions.len()];
    loop {
        direct.set_typed_needs(&needs);
        let mut changed = false;
        for (index, typed) in direct.typed.iter().enumerate() {
            let Some(typed) = typed else {
                continue;
            };
            let mut scratch = String::new();
            emit_typed(&mut scratch, index, typed, false);
            let body = scratch
                .find("kiln_block_0:;")
                .map_or("", |at| &scratch[at..]);
            // A restartable body leaves through its boxed entry on any failed guard, so it
            // relies on nothing a caller must check.
            relies[index] = if typed_restartable[index] {
                Default::default()
            } else {
                emit_graph::typed_relies(body)
            };
            typed_callees[index] = typed_entry_calls(body, symbol);
            let found = direct::TypedNeeds {
                // Instance-global lookup is emitted in the activation prologue, before block 0.
                // Include that host use without counting the host parameter in the signature.
                runtime: names(scratch.split_once('{').map_or("", |(_, body)| body), "host")
                    || names(body, "thrown"),
                upvalues: names(body, "upvalues"),
                receiver: names(body, "receiver"),
                // The typed receiver-row prologue is emitted before block 0 and can consume
                // `recv_row_in` even when the body itself never calls another typed entry.
                // Include that prologue use in the signature fixed point.
                row: names(&scratch, "recv_row_in") || names(body, "recv_row"),
            };
            let merged = direct::TypedNeeds {
                runtime: needs[index].runtime || found.runtime,
                upvalues: needs[index].upvalues || found.upvalues,
                receiver: needs[index].receiver || found.receiver,
                row: needs[index].row || found.row,
            };
            if merged != needs[index] {
                needs[index] = merged;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // A typed callee's speculations are its caller's: a typed caller has no boxed call to take
    // when the callee's guard fails, so the caller's own call site asks for both.
    loop {
        let mut changed = false;
        for index in 0..relies.len() {
            let mut merged = relies[index].clone();
            for &callee in &typed_callees[index] {
                if callee < relies.len() {
                    merged.extend(relies[callee].iter().cloned());
                }
            }
            if merged.len() != relies[index].len() {
                relies[index] = merged;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // A typed recursive call normally reaches its typed entry. When that entry can fall back
    // because one of its guarded built-ins was changed earlier in the call, the fallback calls
    // the generic body using this closure's upvalue table. That table can be needed by the
    // generic body even when the typed fast path itself emitted no upvalue read, so carry it in
    // the typed signature whenever the guarded self-call can take that edge.
    for index in 0..typed_callees.len() {
        if needs[index].runtime
            && typed_callees[index].contains(&index)
            && !relies[index].is_empty()
            && !program.functions[index].upvalues.is_empty()
        {
            needs[index].upvalues = true;
        }
    }
    direct.set_typed_needs(&needs);
    let guards: Vec<String> = relies
        .iter()
        .map(|terms| terms.iter().cloned().collect::<Vec<_>>().join(" && "))
        .collect();
    direct.set_typed_guards(&guards);
    // A restartable body gets a fast clone: decided by emitting it, once, before any caller.
    // The parallel clones (ADR 0058, application 2): for every proto whose body passes
    // `parallel::parallel_safe` and whose typed entry returns a number, emit the typed body once
    // more under `parallel_clone()` -- named `_p`, `Math.<name>` as bare libm -- and admit it only
    // if its text names none of the host, `thrown`, the receiver, the upvalue table, a shape row,
    // or the collector: the same reading `typed_needs` does above, as the machine's own check
    // that a chunk worker calling this clone reaches nothing a realm thread owns.
    //
    // The clone is emitted from the proto's parallel plan (`direct.parallel`), which assumes the
    // captured reads of tier 1 are numbers: the clone takes them as `double` parameters after
    // its own, and the call site guards them on the realm thread before any chunk runs.
    let mut parallel_clones: Vec<Option<String>> = vec![None; program.functions.len()];
    for (index, parallel) in direct.parallel.iter().enumerate() {
        let Some(typed) = parallel else {
            continue;
        };
        if direct.typed[index].is_none() || !typed.parallel_safe[index] {
            continue;
        }
        let proto = &program.functions[index];
        let mut scratch = String::new();
        emit_graph::with_parallel_clone(|| {
            emit_graph::emit_function_from_graph_with_profile(
                &mut scratch,
                &graphs[index],
                proto,
                program,
                symbol,
                index,
                !proto_produces_references(proto, program),
                diagnostic_site,
                &plan.functions,
                strict_functions,
                typed,
                emit_graph::EntryKind::Typed {
                    // A predicate (a boolean on some path) answers boxed, for
                    // `parallelFilter`'s workers alone (`parallel::parallel_result`).
                    returns_number: !emit_graph::parallel_boolean_result(&direct.entry, index),
                    restartable: false,
                },
                None,
                false,
                0,
            );
        });
        let body = scratch
            .find("kiln_block_0:;")
            .map_or("", |at| &scratch[at..]);
        let clean = ![
            "host",
            "thrown",
            "receiver",
            "upvalues",
            "recv_row",
            "ctx",
            "kiln_c99_gc",
            "kiln_c99_new",
            "kiln_c99_alloc",
            "kiln_c99_realm",
            // Module globals are the realm's block (`emit_graph::body::cache_module_globals`),
            // found through the realm generation of the thread that asks: a pool thread has none.
            "kiln_mg",
        ]
        .iter()
        .any(|word| names(body, word));
        if clean {
            parallel_clones[index] = Some(scratch);
        }
    }
    let admitted: Vec<bool> = parallel_clones.iter().map(Option::is_some).collect();
    direct.set_parallel(&admitted);
    // Tier 2 (ADR 0069, parallel issue 07): a callback whose verdict reads the heap gets a heap
    // clone (`_h`, and `_hk` for a key comparator's key), emitted from its graph by the lowering
    // in `parallel_heap.rs`, which knows only what `parallel::heap_verdict` admits.
    let heap_clones: Vec<Option<(String, crate::parallel::HeapShape)>> =
        (0..program.functions.len())
            .map(|index| {
                let Some(Ok(shape)) = direct.entry.parallel.get(index) else {
                    return None;
                };
                let heap = shape.heap.clone()?;
                let proto = &program.functions[index];
                let mut text = String::new();
                let ctx = parallel_heap::CloneContext {
                    symbol,
                    root: index as u32,
                    callee: None,
                };
                // Tier 4: the functions it calls, each cloned for it, ahead of it.
                parallel_heap::emit_heap_callees(
                    &mut text,
                    &graphs,
                    program,
                    symbol,
                    index as u32,
                    &heap,
                );
                parallel_heap::emit_heap_clone(
                    &mut text,
                    &graphs[index],
                    proto,
                    program,
                    &format!("{symbol}_fn_{index}_h"),
                    &heap,
                    None,
                    &ctx,
                );
                if let Some((_, key)) = heap.key {
                    parallel_heap::emit_heap_clone(
                        &mut text,
                        &graphs[index],
                        proto,
                        program,
                        &format!("{symbol}_fn_{index}_hk"),
                        &heap,
                        Some(key),
                        &ctx,
                    );
                }
                Some((text, heap))
            })
            .collect();
    let fast: Vec<bool> = program
        .functions
        .iter()
        .enumerate()
        .map(|(index, proto)| {
            (!policy.generic_only && proto.code.len() <= MAX_SPECIALIZED_INSTRUCTIONS)
                .then(|| {
                    emit_graph::emit_fast_clone(
                        &graphs[index],
                        proto,
                        program,
                        symbol,
                        index,
                        !proto_produces_references(proto, program),
                        diagnostic_site,
                        &plan.functions,
                        strict_functions,
                        &direct.functions[index],
                    )
                })
                .flatten()
                .is_some()
        })
        .collect();
    direct.set_fast(&fast);
    // A call site remembers the callee's receiver-cache row, so the tables are declared
    // before any body: the definitions follow with the boxed clone that owns them.
    for (index, &columns) in receiver_columns.iter().enumerate() {
        if columns > 0 {
            // Chunked, the boxed clone that owns them defines them (in whichever chunk it lands);
            // the unit's own text only declares them.
            let linkage = if should_chunk { "extern " } else { "static " };
            writeln!(
                output,
                "{linkage}const kiln_shape *{symbol}_fn_{index}_recv_shape[4];\n{linkage}uint32_t {symbol}_fn_{index}_recv_slot[4][{columns}];\n{linkage}const kiln_shape *{symbol}_fn_{index}_typed_recv_shape[4];\n{linkage}uint32_t {symbol}_fn_{index}_typed_recv_slot[4][{columns}];"
            )
            .unwrap();
            if should_chunk {
                writeln!(
                    fn_declarations[index],
                    "extern const kiln_shape *{symbol}_fn_{index}_recv_shape[4];\nextern uint32_t {symbol}_fn_{index}_recv_slot[4][{columns}];\nextern const kiln_shape *{symbol}_fn_{index}_typed_recv_shape[4];\nextern uint32_t {symbol}_fn_{index}_typed_recv_slot[4][{columns}];"
                )
                .unwrap();
            }
        }
    }
    for (index, &has_fast) in fast.iter().enumerate() {
        if reachable.is_some_and(|set| !set.contains(&index)) {
            continue;
        }
        let static_kw = if should_chunk { "" } else { "static inline " };
        if policy.compact {
            writeln!(output, "{static_kw}int {symbol}_fn_{index}(KU_PARAMS);").unwrap();
            if has_fast {
                writeln!(output, "{static_kw}int {symbol}_fn_{index}_f(KU_PARAMS);").unwrap();
            }
        } else {
            writeln!(
                output,
                "{static_kw}int {symbol}_fn_{index}(kiln_call_context *ctx, kiln_val * restrict result, kiln_val * restrict upvalues, const kiln_val * restrict receiver_ptr, const kiln_val * restrict args, size_t argc);"
            )
            .unwrap();
            if has_fast {
                writeln!(
                    output,
                    "{static_kw}int {symbol}_fn_{index}_f(kiln_call_context *ctx, kiln_val * restrict result, kiln_val * restrict upvalues, const kiln_val * restrict receiver_ptr, const kiln_val * restrict args, size_t argc);"
                )
                .unwrap();
            }
        }
        if should_chunk {
            if policy.compact {
                writeln!(
                    fn_declarations[index],
                    "int {symbol}_fn_{index}(KU_PARAMS);"
                )
                .unwrap();
                if has_fast {
                    writeln!(
                        fn_declarations[index],
                        "int {symbol}_fn_{index}_f(KU_PARAMS);"
                    )
                    .unwrap();
                }
            } else {
                writeln!(
                    fn_declarations[index],
                    "int {symbol}_fn_{index}(kiln_call_context *ctx, kiln_val * restrict result, kiln_val * restrict upvalues, const kiln_val * restrict receiver_ptr, const kiln_val * restrict args, size_t argc);"
                )
                .unwrap();
                if has_fast {
                    writeln!(
                        fn_declarations[index],
                        "int {symbol}_fn_{index}_f(kiln_call_context *ctx, kiln_val * restrict result, kiln_val * restrict upvalues, const kiln_val * restrict receiver_ptr, const kiln_val * restrict args, size_t argc);"
                    )
                    .unwrap();
                }
            }
        }
        if direct.has_typed_entry(index as u32) {
            let (result, params) = emit_graph::typed_signature(
                &program.functions[index],
                direct.entry.typed_returns[index],
                direct.entry.typed_needs[index],
            );
            writeln!(
                output,
                "{static_kw}{result} {symbol}_fn_{index}_t({params});"
            )
            .unwrap();
            if should_chunk {
                writeln!(
                    fn_declarations[index],
                    "{result} {symbol}_fn_{index}_t({params});"
                )
                .unwrap();
            }
            // Issue 21: the entry's receiver-row lookup, asked by a method call site before it
            // enters the entry (defined beside the entry, `emit_graph/body.rs`).
            if direct.entry.typed_needs[index].row {
                let declaration = format!(
                    "const uint32_t *{symbol}_fn_{index}_t_row(const kiln_object *recv_obj);"
                );
                writeln!(output, "{static_kw}{declaration}").unwrap();
                if should_chunk {
                    writeln!(fn_declarations[index], "{declaration}").unwrap();
                }
            }
            if direct.parallel_chunk(index as u32) {
                // Issue 14: with a worker unit, the chunk workers are defined there, under the
                // module's link-visible symbol, and this unit only declares them. A compact
                // unit names its own functions `ku_*`; a define keeps the call site's spelling.
                let worker_symbol = workers.as_ref().map_or(symbol, |(link, _)| *link);
                let pmap_link = if should_chunk || workers.is_some() {
                    ""
                } else {
                    "static "
                };
                for kind in emit_graph::parallel_chunk_kinds(
                    program.functions[index].parameter_count,
                    emit_graph::parallel_boolean_result(&direct.entry, index),
                ) {
                    writeln!(
                        output,
                        "{pmap_link}void {worker_symbol}_pmap{kind}_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);"
                    )
                    .unwrap();
                    if worker_symbol != symbol {
                        writeln!(
                            output,
                            "#define {symbol}_pmap{kind}_{index} {worker_symbol}_pmap{kind}_{index}"
                        )
                        .unwrap();
                    }
                    if should_chunk {
                        writeln!(
                            fn_declarations[index],
                            "void {worker_symbol}_pmap{kind}_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);"
                        )
                        .unwrap();
                    }
                }
            }
        }
        // Tier 2's workers, declared like tier 1's (issue 14's worker unit included).
        if let Some((_, heap)) = &heap_clones[index] {
            let worker_symbol = workers.as_ref().map_or(symbol, |(link, _)| *link);
            let pmap_link = if should_chunk || workers.is_some() {
                ""
            } else {
                "static "
            };
            for kind in parallel_heap::heap_chunk_kinds(
                program.functions[index].parameter_count,
                heap.key.is_some(),
                heap.allocates,
            ) {
                writeln!(
                    output,
                    "{pmap_link}void {worker_symbol}_pmap{kind}_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);"
                )
                .unwrap();
                if worker_symbol != symbol {
                    writeln!(
                        output,
                        "#define {symbol}_pmap{kind}_{index} {worker_symbol}_pmap{kind}_{index}"
                    )
                    .unwrap();
                }
                if should_chunk {
                    writeln!(
                        fn_declarations[index],
                        "void {worker_symbol}_pmap{kind}_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);"
                    )
                    .unwrap();
                }
            }
        }
    }
    // Function-name statics come from `effective_name_units` in the function-name section
    // below; the closure keeps a pointer to the headed static, never a C-string literal.
    // One copy of the unit's source text, which every closure's `toString` span points into.
    // Emitted whole rather than per function for the reason `Program::source` gives: spans nest,
    // so a copy per function would store an outer body once for every function written inside
    // it. A unit with no function bodies needs none of it.
    if !program.functions.is_empty() && !program.source.is_empty() {
        writeln!(
            output,
            "static const char {symbol}_source[] = {};",
            c_string_chunked(&program.source)
        )
        .unwrap();
    }
    let has_closure = program
        .code
        .iter()
        .any(|i| matches!(i, Instruction::MakeFunction(_)))
        || program.functions.iter().any(|f| {
            f.code
                .iter()
                .any(|i| matches!(i, Instruction::MakeFunction(_)))
        });
    if policy.compact && has_closure && !program.functions.is_empty() {
        writeln!(
            output,
            "typedef struct {{\n\
             kiln_body_fn body;\n\
             const kiln_string *name;\n\
             uint32_t span_start;\n\
             uint32_t span_len;\n\
             uint32_t handlers;\n\
             uint32_t resume;\n\
             uint16_t arity;\n\
             uint16_t locals;\n\
             uint16_t stack;\n\
             uint8_t flags;\n\
             uint8_t _pad[1];\n\
             }} ku_proto_meta;\n\
             static const ku_proto_meta ku_protos[{}] = {{",
            program.functions.len()
        )
        .unwrap();

        for (index, target) in program.functions.iter().enumerate() {
            if reachable.is_some_and(|set| !set.contains(&index)) {
                output.push_str("    { NULL, NULL, 0, 0, 0, 0, 0, 0, 0, 0, {0} },\n");
                continue;
            }
            let name = target.effective_name_units().map_or_else(
                || "NULL".to_owned(),
                |_| format!("&{symbol}_function_name_{index}"),
            );
            let span = target.span;
            let (span_start, span_len) = if program.source.is_empty()
                || policy.omit_source
                || span.end as usize > program.source.len()
                || span.start > span.end
            {
                (0u32, 0u32)
            } else {
                (span.start, span.end - span.start)
            };
            let arity = target.declared_length;
            let locals = target.local_count.max(1);
            let stack = plan.functions.get(index).copied().unwrap_or(1).max(1);
            let handlers = target.code.len().max(1);
            let resume = if binds_at_call(target) {
                target.code.len()
            } else {
                0
            };
            let mut flags: u8 = 0;
            if target.generator || target.asynchronous {
                flags |= 1 << 0;
            }
            if target.asynchronous {
                flags |= 1 << 1;
            }
            if target.async_generator {
                flags |= 1 << 2;
            }
            if target.is_method {
                flags |= 1 << 3;
            }
            if target.is_class {
                flags |= 1 << 7;
            }
            if target.arrow {
                flags |= 1 << 4;
            }
            if strict_functions.get(index).copied().unwrap_or(true) {
                flags |= 1 << 5;
            }
            if direct
                .entry
                .publishes_frame
                .get(index)
                .copied()
                .unwrap_or(true)
            {
                flags |= 1 << 6;
            }
            writeln!(
                output,
                "    {{ (kiln_body_fn){symbol}_fn_{index}, {name}, {span_start}, {span_len}, {handlers}, {resume}, {arity}, {locals}, {stack}, {flags}, {{0}} }},"
            )
            .unwrap();
        }
        writeln!(output, "}};\n").unwrap();

        let source_expr = if !program.source.is_empty() && !policy.omit_source {
            format!("{symbol}_source")
        } else {
            "NULL".to_string()
        };
        let closure_linkage = if should_chunk { "" } else { "static " };
        let closure_fn_name = if should_chunk {
            format!("{symbol}_make_closure")
        } else {
            "ku_make_closure".to_string()
        };
        writeln!(
            output,
            "{closure_linkage}__attribute__((unused)) int {closure_fn_name}(kiln_val *dst, uint32_t proto, kiln_val receiver, const kiln_val *captured, uint32_t count) {{\n\
             const ku_proto_meta *m = &ku_protos[proto];\n\
             kiln_value b;\n\
             const char *src = m->span_len ? ({source_expr} + m->span_start) : NULL;\n\
             int status = kiln_c99_make_closure(\n\
             &b,\n\
             m->body,\n\
             m->name,\n\
             src,\n\
             (size_t)m->span_len,\n\
             ku_site,\n\
             proto,\n\
             (uint32_t)m->arity,\n\
             (m->flags & 1) != 0,\n\
             (m->flags & 2) != 0,\n\
             (m->flags & 4) != 0,\n\
             (m->flags & 128) != 0 ? KILN_C99_FUNCTION_CLASS : ((m->flags & 8) != 0 ? KILN_C99_FUNCTION_METHOD : KILN_C99_FUNCTION_ORDINARY),\n\
             (m->flags & 16) != 0 ? ((m->flags & 64) != 0 ? 1 : 2) : 0,\n\
             (m->flags & 32) != 0,\n\
             receiver,\n\
             kiln_val_from_boundary(kiln_c99_new_target),\n\
             (uint32_t)m->locals,\n\
             (uint32_t)m->stack,\n\
             (uint32_t)m->handlers,\n\
             (uint32_t)m->resume,\n\
             captured,\n\
             count\n\
             );\n\
             if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;\n\
             if (!(m->flags & 64)) kiln_c99_closure_of(b)->publishes_frame = 0;\n\
             *dst = kiln_val_from_boundary(b);\n\
             return KILN_C99_OK;\n\
             }}\n"
        )
        .unwrap();
        // A closure-making site in the shared immutable region (realms issue 08): the same
        // metadata, and the site's process-wide slot.
        if module_cells::region_site_count() > 0 {
            writeln!(
                output,
                "{closure_linkage}__attribute__((unused)) int {closure_fn_name}_region(kiln_val *dst, uint32_t proto, kiln_val receiver, kiln_closure **site) {{\n\
                 const ku_proto_meta *m = &ku_protos[proto];\n\
                 kiln_value b;\n\
                 const char *src = m->span_len ? ({source_expr} + m->span_start) : NULL;\n\
                 int status = kiln_c99_region_closure(&b, site, m->body, m->name, src, (size_t)m->span_len, ku_site, proto, (uint32_t)m->arity, (m->flags & 1) != 0, (m->flags & 2) != 0, (m->flags & 4) != 0, (m->flags & 128) != 0 ? KILN_C99_FUNCTION_CLASS : ((m->flags & 8) != 0 ? KILN_C99_FUNCTION_METHOD : KILN_C99_FUNCTION_ORDINARY), (m->flags & 16) != 0 ? ((m->flags & 64) != 0 ? 1 : 2) : 0, (m->flags & 32) != 0, receiver, (uint32_t)m->locals, (uint32_t)m->stack, (uint32_t)m->handlers, (uint32_t)m->resume, (m->flags & 64) != 0);\n\
                 if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;\n\
                 *dst = kiln_val_from_boundary(b);\n\
                 return KILN_C99_OK;\n\
                 }}\n"
            )
            .unwrap();
        }
    }
    if should_chunk {
        output.push_str("/* KILN_CHUNK_0_FUNCTIONS_PLACEHOLDER */\n");
    }
    let mut site_offset = 0usize;
    for (index, proto) in program.functions.iter().enumerate() {
        if reachable.is_some_and(|set| !set.contains(&index)) {
            continue;
        }
        let mut func_buf = String::new();
        let target_buf = if should_chunk {
            &mut func_buf
        } else {
            &mut *output
        };
        let scalar_proto = !proto_produces_references(proto, program);
        let func_profile = policy.type_profile.as_ref().and_then(|p| p.function(index));
        // The boxed entry declares the callee caches its own plan's typed sites need. The typed
        // clone below is emitted from a different plan (`direct.typed`), whose typed entries can
        // include a candidate the boxed plan's do not, and it names the same
        // `{symbol}_fn_{index}_v{v}_t_{i}` statics. Declare the difference here, ahead of both,
        // so a clone never references a cache only the other plan would have declared.
        if let Some(typed) = &direct.typed[index] {
            let boxed_sites = emit_graph::sites::collect_typed_call_sites(
                &graphs[index],
                program,
                &direct.functions[index],
                &direct.functions[index].numbers,
            );
            for (vid, candidates) in emit_graph::sites::collect_typed_call_sites(
                &graphs[index],
                program,
                typed,
                &typed.numbers,
            ) {
                for i in candidates {
                    let declared = boxed_sites
                        .iter()
                        .any(|(v, cs)| *v == vid && cs.contains(&i));
                    if declared {
                        continue;
                    }
                    let pfx = format!("{symbol}_fn_{index}_v{}_t_{i}", vid.0);
                    writeln!(
                        target_buf,
                        "static kiln_c99_method_path {pfx}_path = KILN_C99_UNLEARNED_METHOD_PATH;"
                    )
                    .unwrap();
                }
            }
        }
        let sites_count = emit_graph::emit_function_from_graph_with_profile(
            target_buf,
            &graphs[index],
            proto,
            program,
            symbol,
            index,
            scalar_proto,
            diagnostic_site,
            &plan.functions,
            strict_functions,
            &direct.functions[index],
            emit_graph::EntryKind::Boxed,
            func_profile,
            policy.type_profile_record,
            site_offset,
        );
        site_offset += sites_count;
        if let Some(typed) = &direct.typed[index] {
            emit_typed(target_buf, index, typed, true);
            if let Some(clone) = &parallel_clones[index] {
                let captures = emit_graph::parallel_captures(&direct.entry, index);
                if let Some((link, unit)) = workers.as_mut() {
                    // Issue 14: the clone and its chunk workers go to the worker unit, which
                    // is compiled optimised; the workers are link-visible under the module's
                    // own symbol, the clone stays private to that unit.
                    unit.push_str(clone);
                    emit_graph::emit_parallel_chunk(
                        unit,
                        link,
                        symbol,
                        index,
                        proto.parameter_count,
                        emit_graph::parallel_boolean_result(&direct.entry, index),
                        &captures,
                        true,
                    );
                } else {
                    target_buf.push_str(clone);
                    emit_graph::emit_parallel_chunk(
                        target_buf,
                        symbol,
                        symbol,
                        index,
                        proto.parameter_count,
                        emit_graph::parallel_boolean_result(&direct.entry, index),
                        &captures,
                        should_chunk,
                    );
                }
            }
        }
        // Tier 2: the heap clone and its chunk workers, where tier 1's go (issue 14's worker
        // unit, compiled optimised, when there is one). The workers reach the heap only through
        // the prelude's `kiln_c99_par_*` readers, which the worker unit's header declares.
        if let Some((clone, heap)) = &heap_clones[index] {
            let kinds = parallel_heap::heap_chunk_kinds(
                proto.parameter_count,
                heap.key.is_some(),
                heap.allocates,
            );
            if let Some((link, unit)) = workers.as_mut() {
                unit.push_str(clone);
                parallel_heap::emit_heap_workers(
                    unit,
                    link,
                    symbol,
                    index,
                    proto.parameter_count,
                    &kinds,
                    heap.sites,
                    "",
                );
            } else {
                target_buf.push_str(clone);
                parallel_heap::emit_heap_workers(
                    target_buf,
                    symbol,
                    symbol,
                    index,
                    proto.parameter_count,
                    &kinds,
                    heap.sites,
                    if should_chunk { "" } else { "static " },
                );
            }
        }
        // Emitted again now that every plan knows the fast set: a fast clone calls other fast
        // clones by name.
        if fast[index]
            && let Some(clone) = emit_graph::emit_fast_clone(
                &graphs[index],
                proto,
                program,
                symbol,
                index,
                scalar_proto,
                diagnostic_site,
                &plan.functions,
                strict_functions,
                &direct.functions[index],
            )
        {
            target_buf.push_str(&clone);
        }
        if should_chunk {
            if let Some(ref mut bodies) = chunked_bodies {
                bodies.push(func_buf);
            }
        }
    }
    if policy.type_profile_record {
        writeln!(output, "static int {symbol}_profile_dumped = 0;").unwrap();
        writeln!(output, "static void {symbol}_dump_type_profile(void) {{").unwrap();
        writeln!(output, "    if ({symbol}_profile_dumped) return;").unwrap();
        writeln!(output, "    {symbol}_profile_dumped = 1;").unwrap();
        let default_file = if diagnostic_site.is_empty() {
            "kiln-native.typeprofile".to_owned()
        } else {
            let base = diagnostic_site
                .strip_suffix(".mjs")
                .or_else(|| diagnostic_site.strip_suffix(".js"))
                .or_else(|| diagnostic_site.strip_suffix(".ts"))
                .unwrap_or(diagnostic_site);
            format!("{base}.typeprofile")
        };
        writeln!(
            output,
            "    const char *path = getenv(\"KILN_TYPE_PROFILE_FILE\");"
        )
        .unwrap();
        writeln!(
            output,
            "    if (!path || !path[0]) path = \"{default_file}\";"
        )
        .unwrap();
        writeln!(output, "    FILE *f = fopen(path, \"w\");").unwrap();
        writeln!(output, "    if (!f) return;").unwrap();
        writeln!(
            output,
            "    fprintf(f, \"{{\\n  \\\"version\\\": 1,\\n  \\\"functions\\\": [\\n\");"
        )
        .unwrap();
        for (index, proto) in program.functions.iter().enumerate() {
            let is_last = index + 1 == program.functions.len();
            writeln!(
                output,
                "    fprintf(f, \"    {{\\n      \\\"index\\\": {index},\\n\");"
            )
            .unwrap();
            if let Some(name) = &proto.name {
                let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
                writeln!(
                    output,
                    "    fprintf(f, \"      \\\"name\\\": \\\"{escaped}\\\",\\n\");"
                )
                .unwrap();
            }
            writeln!(output, "    fprintf(f, \"      \\\"parameters\\\": [\");").unwrap();
            for p in 0..proto.parameter_count {
                let comma = if p > 0 { ", " } else { "" };
                writeln!(
                    output,
                    "    fprintf(f, \"{comma}\\\"%s\\\"\", kiln_profile_tag_name({symbol}_fn_{index}_param_tags[{p}]));"
                ).unwrap();
            }
            writeln!(output, "    fprintf(f, \"],\\n\");").unwrap();
            writeln!(
                output,
                "    fprintf(f, \"      \\\"return_type\\\": \\\"%s\\\"\\n    }}{}\\n\", kiln_profile_tag_name({symbol}_fn_{index}_return_tag));",
                if is_last { "" } else { "," }
            ).unwrap();
        }
        writeln!(
            output,
            "    fprintf(f, \"  ],\\n  \\\"sites\\\": []\\n}}\\n\");"
        )
        .unwrap();
        writeln!(output, "    fclose(f);").unwrap();
        writeln!(output, "}}\n").unwrap();
    }
    RenderedFunctions {
        sites: site_offset,
        function_declarations: fn_declarations,
    }
}

/// Whether this body's parameter binding runs during the call rather than at the first
/// resumption.
///
/// Only a suspending body has the two halves apart at all: an ordinary call runs them together
/// and never needs the seam. An async *function* is the one suspending shape that must keep them
/// together too -- its instantiation failure rejects rather than throwing, and running the
/// prologue as the first thing its driver does is already exactly that.
pub(crate) fn binds_at_call(proto: &FunctionProto) -> bool {
    (proto.generator || proto.asynchronous)
        && !(proto.asynchronous && !proto.async_generator)
        && proto.prologue_end > 0
        && (proto.prologue_end as usize) < proto.code.len()
}

/// Whether `render_function` may lower a body through the SSA value graph (ADR 0056).
///
/// Off until the graph merges locals at control-flow joins. Today `build_function_graph` reuses
/// the SSA value it last knew for a local across a back edge without a phi, so a loop header
/// compares the pre-loop value forever: `const run = () => { for (let i = 0; i < 5; i++) {} }`
/// compiled to an infinite loop on 2026-09-05 (`conformance/native/for-loop-in-arrow.js`).
/// A constant rather than an environment variable because emitted C is cached by source and
/// options; emission that varied with the environment under one key would poison the cache.
/// The unit tests drive `build_function_graph` and `emit_function_from_graph` directly and do
/// not go through this switch.
/// The value graph a body is emitted from: built from its bytecode, checked to lower whole, and
/// verified. There is no other emitter, so each failure is the unit's: an instruction the builder
/// has no lowering for is `Unsupported`, and a graph the verifier rejects is a bytecode this
/// backend cannot represent safely, which is what `InvalidProgram` names. Errors rather than
/// panics, so a daemon fails one build and not itself.
fn graph_for(
    proto: &FunctionProto,
    program: &Program,
    what: &str,
    diagnostic_site: &str,
) -> Result<FunctionGraph, EmitError> {
    let graph =
        build_function_graph(proto, program).map_err(|unsupported| EmitError::Unsupported {
            feature: format!(
                "{} at instruction {} of {what} in {diagnostic_site}",
                unsupported.instruction, unsupported.pc
            ),
        })?;
    if let Some(operation) = emit_graph::first_unlowerable(&graph) {
        return Err(EmitError::Unsupported {
            feature: format!("{operation} in {what} in {diagnostic_site}"),
        });
    }
    verifier::verify_function_graph(&graph).map_err(|error| EmitError::InvalidProgram {
        message: format!(
            "value graph rejected by the verifier for {what} ({}) in {diagnostic_site}: {error}",
            proto.name.as_deref().unwrap_or("<anonymous>")
        ),
    })?;
    Ok(graph)
}

/// The unit's top-level code as a body the graph builder takes: no parameters, every slot
/// starting undefined, and nothing captured.
fn entry_proto(program: &Program) -> FunctionProto {
    FunctionProto {
        inferred_name: None,
        asynchronous: has_top_level_await(program),
        name: None,
        arrow: false,
        generator: false,
        async_generator: false,
        is_method: false,
        is_class: false,
        parameter_count: 0,
        mapped_arguments: false,
        declared_length: 0,
        local_count: program.local_count,
        prologue_end: 0,
        code: program.code.clone(),
        upvalues: Vec::new(),
        span: kiln_ir::Span::default(),
        type_annotations: None,
    }
}

/// Replace direct-eval instructions in bytecode-unreachable regions with Nop and report whether
/// any reachable direct eval remains. Both conditional paths and registered exception handlers
/// are treated as reachable; only unconditional control flow proves a site dead.
fn prune_unreachable_direct_eval(program: &mut Program) -> bool {
    fn prune_body(code: &mut [Instruction]) {
        let mut reachable = HashSet::new();
        let mut pending = vec![0usize];
        while let Some(pc) = pending.pop() {
            if pc >= code.len() || !reachable.insert(pc) {
                continue;
            }
            let next = pc + 1;
            match &code[pc] {
                Instruction::Jump(target) => pending.push(*target as usize),
                Instruction::JumpIfFalse(target)
                | Instruction::JumpIfTruthy(target)
                | Instruction::JumpIfNotNullish(target)
                | Instruction::JumpIfReturnSignal(target) => {
                    pending.push(*target as usize);
                    pending.push(next);
                }
                // A handler can be entered by any throwing operation in the protected region.
                // Conservatively count it as reachable as soon as its registration executes.
                Instruction::PushHandler(target) => {
                    pending.push(*target as usize);
                    pending.push(next);
                }
                Instruction::Return | Instruction::Throw => {}
                // YieldStar carries the resumption target in addition to its ordinary
                // continuation. Mark both to avoid pruning eval code entered by resumption.
                Instruction::YieldStar(target) => {
                    pending.push(*target as usize);
                    pending.push(next);
                }
                _ => pending.push(next),
            }
        }
        for (pc, instruction) in code.iter_mut().enumerate() {
            if matches!(instruction, Instruction::DirectEval(_)) && !reachable.contains(&pc) {
                *instruction = Instruction::Nop;
            }
        }
    }

    prune_body(&mut program.code);
    for function in &mut program.functions {
        prune_body(&mut function.code);
    }
    program.has_direct_eval()
}

/// The typed entries of this unit that `body` calls: every `{symbol}_fn_{index}_t(`.
fn typed_entry_calls(body: &str, symbol: &str) -> std::collections::BTreeSet<usize> {
    let prefix = format!("{symbol}_fn_");
    let mut found = std::collections::BTreeSet::new();
    let mut from = 0;
    while let Some(at) = body[from..].find(&prefix) {
        let start = from + at + prefix.len();
        let digits = body[start..].bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0
            && body[start + digits..].starts_with("_t(")
            && let Ok(index) = body[start..start + digits].parse()
        {
            found.insert(index);
        }
        from = start;
    }
    found
}

/// Whether emitted C names `word` as an identifier of its own: not as part of a longer name,
/// and not as a struct field behind `->`.
fn names(text: &str, word: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(at) = text[from..].find(word) {
        let start = from + at;
        let end = start + word.len();
        let before = start.checked_sub(1).map(|i| bytes[i]);
        let after = bytes.get(end).copied();
        let is_ident = |b: Option<u8>| b.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_');
        let field = start >= 2 && &text[start - 2..start] == "->";
        if !is_ident(before) && !is_ident(after) && !field {
            return true;
        }
        from = end;
    }
    false
}

/// Every body's graph, built and verified before any is emitted, and the unit's direct-call
/// plan over all of them: which calls are by name, which enter a typed clone, and what each
/// body proves a number once those verdicts are folded in.
fn plan_unit(
    program: &Program,
    diagnostic_site: &str,
    compact: bool,
    compile_regex: bool,
    generic_only: bool,
    parallel_in_generic: bool,
    speculate: Option<bool>,
    extra_entry_slots: Option<&HashSet<u32>>,
    generic_loop_profile: Option<Arc<direct::LoopCallProfile>>,
) -> Result<
    (
        Vec<FunctionGraph>,
        FunctionProto,
        FunctionGraph,
        direct::DirectPlan,
    ),
    EmitError,
> {
    let graphs = program
        .functions
        .iter()
        .enumerate()
        .map(|(index, proto)| {
            graph_for(
                proto,
                program,
                &format!("function {index}"),
                diagnostic_site,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let body = entry_proto(program);
    let mut entry_graph = graph_for(&body, program, "the module body", diagnostic_site)?;
    if let Some(extra) = extra_entry_slots {
        entry_graph.mark_non_escaping_objects_with_extra_slots(program, Some(extra));
    }
    let argument_analysis = kiln_graph::ArgumentsAnalysis::new(program);
    let argument_facts: Vec<emit_graph::ArgumentsLengthFact> = graphs
        .iter()
        .enumerate()
        .map(|(index, graph)| argument_analysis.argument_length_fact(index, graph))
        .collect();
    let arguments_length_only: Vec<bool> = argument_facts
        .iter()
        .map(|fact| !fact.values.is_empty())
        .collect();
    let publishes_frame = compute_publishes_frame_with_argument_facts(program, &argument_facts);
    let mut direct = direct::DirectPlan::plan_with_argument_length_facts(
        program,
        &graphs,
        &entry_graph,
        &publishes_frame,
        compact,
        compile_regex,
        generic_only,
        parallel_in_generic,
        speculate,
        generic_loop_profile,
        &arguments_length_only,
    );
    let empty_argument_values = std::sync::Arc::new(HashSet::new());
    for (index, fact) in argument_facts.iter().enumerate() {
        let values = if fact.values.is_empty() {
            empty_argument_values.clone()
        } else {
            std::sync::Arc::new(fact.values.clone())
        };
        direct.functions[index].argument_length_values = Some(values.clone());
        if let Some(plan) = &mut direct.typed[index] {
            plan.argument_length_values = Some(values.clone());
        }
        if let Some(plan) = &mut direct.parallel[index] {
            plan.argument_length_values = Some(values);
        }
    }
    Ok((graphs, body, entry_graph, direct))
}

/// The unit's entry point: opened with the embedding signature, its locals declared and the
/// imports bound into them, then the body from its graph.
///
fn render_module_local_reset(
    output: &mut String,
    unit_symbol: &str,
    locals: usize,
    module: Option<(&ModuleProgram, bool)>,
    preserve_export_cells: bool,
) {
    if preserve_export_cells {
        return;
    }
    let exports = module
        .filter(|(_, commonjs)| !commonjs && preserve_export_cells)
        .map(|(module, _)| {
            module
                .exports
                .iter()
                .map(|binding| binding.local_slot)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    if exports.is_empty() {
        writeln!(
            output,
            "    for (size_t slot = 0; slot < (size_t){locals}; ++slot) locals[slot] = KILN_VAL_UNDEFINED;"
        )
        .unwrap();
        return;
    }
    writeln!(
        output,
        "    for (size_t slot = 0; slot < (size_t){locals}; ++slot) {{ switch (slot) {{"
    )
    .unwrap();
    for slot in exports {
        writeln!(
            output,
            "        case {slot}: if ((locals[{slot}] & KILN_TAG_MASK) != KILN_TAG_CELL_PREFIX) locals[{slot}] = KILN_VAL_UNDEFINED; break;"
        )
        .unwrap();
    }
    output.push_str("        default: locals[slot] = KILN_VAL_UNDEFINED; break;\n    } }\n");
    let _ = unit_symbol;
}

/// The locals are the unit's globals -- named rather than automatic -- whenever a function reads
/// module scope, because an exported closure reads it after the entry point has returned, and a
/// frame that has returned is not somewhere to read from. Every slot then lives in the array.
/// Otherwise only the slots read through the array do: an import, bound here before the body
/// runs; an export, read from its slot at the return; and what the body's own graph needs.
#[allow(clippy::too_many_arguments)]
fn render_entry(
    output: &mut String,
    program: &Program,
    plan: &StackPlan,
    public_symbol: &str,
    unit_symbol: &str,
    diagnostic_site: &str,
    strict_functions: &[bool],
    keyed: bool,
    module: Option<(&ModuleProgram, bool)>,
    body: &FunctionProto,
    graph: &FunctionGraph,
    calls: &direct::CallPlan,
    policy: &EmissionPolicy,
    preserve_export_cells: bool,
    site_offset: usize,
) -> Result<usize, EmitError> {
    if body.asynchronous {
        let entry_start = output.len();
        let entry_name = format!("{unit_symbol}_entry_name");
        render_static_string(output, &entry_name, &kiln_ir::StringUnits::from("<entry>"));
        let shared_sites = emit_graph::emit_async_entry_body(
            output,
            graph,
            body,
            program,
            unit_symbol,
            diagnostic_site,
            &plan.functions,
            strict_functions,
            calls,
            policy.type_profile_record,
            site_offset,
        );
        if policy.compact && shared_sites > 0 {
            output.insert_str(
                entry_start,
                &format!(
                    "#define {unit_symbol}_entry_site (&{unit_symbol}_sites[{site_offset}])\n"
                ),
            );
        }

        let _callables = true;
        let entry_symbol = format!("{unit_symbol}_body");
        let linkage = "static ";
        match module {
            None => writeln!(
                output,
                "{linkage}int {entry_symbol}(kiln_host *host, kiln_value *result) {{"
            )
            .unwrap(),
            Some(_) => writeln!(
                output,
                "{linkage}int {entry_symbol}(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count) {{"
            )
            .unwrap(),
        }
        writeln!(
            output,
            "    if (kiln_realm_is_active()) {{ int roots_status = kiln_realm_gc_register_roots_with_visitor({unit_symbol}_gc_roots, {unit_symbol}_gc_visit_roots, NULL, 0); if (roots_status != KILN_C99_OK) return roots_status; }}"
        )
        .unwrap();
        render_key_prologue(output, unit_symbol, keyed);
        if policy.type_profile_record {
            writeln!(output, "    (void)atexit({unit_symbol}_dump_type_profile);").unwrap();
        }
        output.push_str("    kiln_val receiver = KILN_VAL_UNDEFINED; (void)receiver;\n");
        output.push_str("    int depth = 0;\n");
        output.push_str("    kiln_value thrown_value = kiln_c99_undefined();\n");
        output.push_str("    kiln_value *thrown = &thrown_value;\n");
        let locals = usize::try_from(program.local_count).unwrap().max(1);
        writeln!(output, "    kiln_val *locals = {unit_symbol}_globals;").unwrap();
        output.push_str("    const void *locals_owner = NULL;\n    (void)locals_owner;\n");
        output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");

        if let Some((module_prog, is_commonjs)) = module {
            let export_count = if is_commonjs {
                1
            } else {
                module_prog.exports.len()
            };
            writeln!(
                output,
                "    if (import_count != (size_t){} || (import_count != 0 && imports == NULL)) return KILN_C99_INVALID_ARGUMENT;",
                module_prog.imports.len()
            )
            .unwrap();
            writeln!(
                output,
                "    if (export_count != (size_t){export_count} || (export_count != 0 && exports == NULL)) return KILN_C99_INVALID_ARGUMENT;"
            )
            .unwrap();
        }

        render_module_local_reset(output, unit_symbol, locals, module, preserve_export_cells);

        if let Some((module_prog, _)) = module {
            let boxed_imports = slots_read_as_cells(&program.code, &program.functions);
            for (index, binding) in module_prog.imports.iter().enumerate() {
                let slot = binding.local_slot;
                writeln!(
                    output,
                    "    {{ kiln_val imported = kiln_val_from_boundary(imports[{index}]); if ((locals[{slot}] & KILN_TAG_MASK) == KILN_TAG_CELL_PREFIX && (imported & KILN_TAG_MASK) != KILN_TAG_CELL_PREFIX) kiln_c99_val_store_cell((kiln_val *)KILN_VAL_PTR(locals[{slot}]), imported); else locals[{slot}] = imported; }}"
                )
                .unwrap();
                if boxed_imports.contains(&slot) {
                    writeln!(
                        output,
                        "    if ((locals[{slot}] & KILN_TAG_MASK) != KILN_TAG_CELL_PREFIX) {{ int cell_status = kiln_c99_val_make_cell_owned(&locals[{slot}], host != NULL ? host->module_owner : NULL); if (cell_status != KILN_C99_OK) return cell_status; }}"
                    )
                    .unwrap();
                }
            }
        }

        let frame_stack = plan.body.max(1);
        let frame_handlers = program.code.len().max(1);
        let diagnostic_source = diagnostic_source(diagnostic_site);
        writeln!(
            output,
            r#"    kiln_value entry_closure;
    /* The entry name is a borrowed headed static, like `static_text`: ASCII bytes are
     * Latin1 units, and the literal outlives the closure that keeps the pointer. */
    static const kiln_string {unit_symbol}_entry_name = {{ 7, KILN_STRING_LATIN1, "<entry>" }};
    int status = kiln_c99_make_closure(
        &entry_closure,
        {unit_symbol}_entry_body,
        &{unit_symbol}_entry_name,
        "<entry>", 7,
        {diagnostic_source},
        0, 0,
        1, 1, 0, 0, 0, 1,
        KILN_VAL_UNDEFINED,
        KILN_VAL_UNDEFINED,
        UINT32_C({locals}),
        UINT32_C({frame_stack}),
        UINT32_C({frame_handlers}),
        0, NULL, 0
    );
    if (status != KILN_C99_OK) return status;
    kiln_c99_closure_of(entry_closure)->module_host = kiln_c99_module_host(host);
    kiln_c99_closure_set_external_owner(kiln_c99_closure_of(entry_closure),
                                        host != NULL ? host->module_owner : NULL);
    status = kiln_c99_call(host, result, entry_closure, kiln_c99_undefined(), NULL, 0, depth, thrown);
    if (status == KILN_C99_THROWN) {{
        *result = kiln_c99_text(*thrown);
        return KILN_C99_THROWN;
    }}
    if (status != KILN_C99_OK) return status;"#
        )
        .unwrap();

        if let Some((module_prog, is_commonjs)) = module {
            if is_commonjs {
                writeln!(
                    output,
                    "    {{ kiln_val mod_val = locals[2]; kiln_val module_val = (mod_val & KILN_TAG_MASK) == KILN_TAG_CELL_PREFIX ? *(kiln_val *)KILN_VAL_PTR(mod_val) : mod_val; int export_status = kiln_c99_get(host, kiln_val_to_boundary(module_val), \"exports\", 7, &exports[0], depth, thrown); if (export_status != KILN_C99_OK) return export_status; }}"
                )
                .unwrap();
            } else {
                for (index, binding) in module_prog.exports.iter().enumerate() {
                    writeln!(
                        output,
                        "    exports[{index}] = kiln_val_binding_to_boundary(locals[{}]);",
                        binding.local_slot
                    )
                    .unwrap();
                }
            }
        }
        output.push_str("    return KILN_C99_OK;\n}\n");
        render_publish_exports(output, public_symbol, unit_symbol, module, true);
        return Ok(shared_sites);
    }

    let entry_start = output.len();
    // A split module phase calls the private `_body` symbol from the realm wrapper even when
    // this entry has no closures of its own. Keep its naming/linkage decision aligned with the
    // module-level `callables` policy above or the instantiation wrapper links an undefined body.
    let callables = produces_references(program)
        || (policy.compact && module.is_some())
        || (preserve_export_cells && module.is_some());
    let entry_symbol = if callables {
        format!("{unit_symbol}_body")
    } else {
        unit_symbol.to_owned()
    };
    let module_scope = uses_module_scope(program) || (preserve_export_cells && module.is_some());
    let linkage = if callables { "static " } else { "" };
    match module {
        None => writeln!(
            output,
            "{linkage}int {entry_symbol}(kiln_host *host, kiln_value *result) {{"
        )
        .unwrap(),
        Some(_) => writeln!(
            output,
            "{linkage}int {entry_symbol}(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count) {{"
        )
        .unwrap(),
    }
    writeln!(
        output,
        "    if (kiln_realm_is_active()) {{ int roots_status = kiln_realm_gc_register_roots_with_visitor({unit_symbol}_gc_roots, {unit_symbol}_gc_visit_roots, NULL, 0); if (roots_status != KILN_C99_OK) return roots_status; }}"
    )
    .unwrap();
    render_key_prologue(output, unit_symbol, keyed);
    if policy.type_profile_record {
        writeln!(output, "    (void)atexit({unit_symbol}_dump_type_profile);").unwrap();
    }
    let context_start = output.len();
    // A module's `this` is undefined: a module is strict, and strict mode does not substitute
    // the global object. An arrow made here snapshots it, so it is declared for that read too.
    output.push_str("    kiln_val receiver = KILN_VAL_UNDEFINED; (void)receiver;\n");
    // The entry point owns the depth counter and the thrown value every body writes into. Both
    // are locals rather than parameters because the embedding signature is fixed and a reference
    // must not cross it: an uncaught throw leaves as text in `*result`.
    if callables {
        output.push_str("    int depth = 0;\n");
        output.push_str("    kiln_value thrown_value = kiln_c99_undefined();\n");
        output.push_str("    kiln_value *thrown = &thrown_value;\n");
    }
    output.push_str(
        "    const int kiln_inheriting = kiln_c99_inheriting_new_target; (void)kiln_inheriting;\n",
    );
    let context_end = output.len();
    let mut array_slots: HashSet<u32> = if module_scope {
        (0..program.local_count).collect()
    } else {
        emit_graph::array_slots(graph, program)
    };
    let mut exports: Option<&[kiln_ir::ExportBinding]> = None;
    let mut commonjs = false;
    if let Some((module, is_commonjs)) = module {
        commonjs = is_commonjs;
        exports = Some(&module.exports);
        array_slots.extend(module.imports.iter().map(|binding| binding.local_slot));
        array_slots.extend(module.exports.iter().map(|binding| binding.local_slot));
        if is_commonjs {
            array_slots.insert(2);
        }
    }
    let max_slot = array_slots
        .iter()
        .copied()
        .max()
        .map(|s| (s as usize) + 1)
        .unwrap_or(0);
    let locals = usize::try_from(program.local_count)
        .unwrap()
        .max(max_slot)
        .max(1);
    if module_scope {
        writeln!(output, "    kiln_val *locals = {unit_symbol}_globals;").unwrap();
        output.push_str("    const void *locals_owner = NULL;\n    (void)locals_owner;\n");
    } else if !array_slots.is_empty() {
        writeln!(output, "    kiln_val locals[{locals}];").unwrap();
        output.push_str("    const void *locals_owner = NULL;\n    (void)locals_owner;\n");
    }
    output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
    match module {
        None => output.push_str("    kiln_c99_reset_diagnostics();\n"),
        Some((module, is_commonjs)) => {
            let export_count = if is_commonjs { 1 } else { module.exports.len() };
            writeln!(
                output,
                "    if (import_count != (size_t){} || (import_count != 0 && imports == NULL)) return KILN_C99_INVALID_ARGUMENT;",
                module.imports.len()
            )
            .unwrap();
            writeln!(
                output,
                "    if (export_count != (size_t){export_count} || (export_count != 0 && exports == NULL)) return KILN_C99_INVALID_ARGUMENT;"
            )
            .unwrap();
        }
    }
    if module_scope || !array_slots.is_empty() {
        render_module_local_reset(output, unit_symbol, locals, module, preserve_export_cells);
    }
    if let Some((module, _)) = module {
        let boxed_imports = slots_read_as_cells(&program.code, &program.functions);
        for (index, binding) in module.imports.iter().enumerate() {
            let slot = binding.local_slot;
            writeln!(
                output,
                "    {{ kiln_val imported = kiln_val_from_boundary(imports[{index}]); if ((locals[{slot}] & KILN_TAG_MASK) == KILN_TAG_CELL_PREFIX && (imported & KILN_TAG_MASK) != KILN_TAG_CELL_PREFIX) kiln_c99_val_store_cell((kiln_val *)KILN_VAL_PTR(locals[{slot}]), imported); else locals[{slot}] = imported; }}"
            )
            .unwrap();
            // The exporter boxes only what can change, so an immutable export arrives as a plain
            // value where the body expects a cell. Wrapping it here is the same thing
            // `execute_module` does on the interpreter side, and for the same reason: the body
            // has one way to read an import and the linker has two ways to deliver one.
            if boxed_imports.contains(&slot) {
                writeln!(
                    output,
                    "    if ((locals[{slot}] & KILN_TAG_MASK) != KILN_TAG_CELL_PREFIX && kiln_c99_val_make_cell(&locals[{slot}]) != KILN_C99_OK) return KILN_C99_OUT_OF_MEMORY;"
                )
                .unwrap();
            }
        }
    }
    let shared_sites = emit_graph::emit_entry_from_graph_with_profile(
        output,
        graph,
        body,
        program,
        unit_symbol,
        !callables,
        diagnostic_site,
        &plan.functions,
        strict_functions,
        &array_slots,
        exports,
        commonjs,
        calls,
        policy.type_profile_record,
        site_offset,
    );
    output.push_str("}\n");
    emit_graph::retain_used_entry_context(output, context_start..context_end);
    if policy.compact && shared_sites > 0 {
        output.insert_str(
            entry_start,
            &format!("#define {unit_symbol}_entry_site (&{unit_symbol}_sites[{site_offset}])\n"),
        );
    }
    render_publish_exports(output, public_symbol, unit_symbol, module, module_scope);
    Ok(shared_sites)
}

fn render_publish_exports(
    output: &mut String,
    public_symbol: &str,
    unit_symbol: &str,
    module: Option<(&ModuleProgram, bool)>,
    module_scope: bool,
) {
    if let Some((module_prog, false)) = module {
        if module_scope {
            writeln!(
                output,
                "void {public_symbol}_publish_exports(kiln_host *host, kiln_value *exports) {{\n    if (exports == NULL) return;"
            )
            .unwrap();
            for (index, binding) in module_prog.exports.iter().enumerate() {
                writeln!(
                    output,
                    "    exports[{index}] = kiln_val_binding_to_boundary({unit_symbol}_globals[{}]);",
                    binding.local_slot
                )
                .unwrap();
            }
            writeln!(output, "}}\n").unwrap();
        }
    }
}

/// The constant table.
///
/// A string is its bytes. A BigInt is its digits plus a slot for the handle the provider builds
/// from them on first use: parsing is not free and a literal's value never changes, so the unit
/// pays for it once however often the literal is loaded.
fn render_static_string(output: &mut String, name: &str, value: &kiln_ir::StringUnits) {
    render_static_string_with_linkage(output, name, value, false);
}

fn render_static_string_with_linkage(
    output: &mut String,
    name: &str,
    value: &kiln_ir::StringUnits,
    external: bool,
) {
    let units: Vec<u16> = value.units().collect();
    let latin1 = units.iter().all(|unit| *unit <= 0xff);
    let linkage = if external { "" } else { "static " };
    if latin1 {
        let payload = if units.is_empty() {
            "0".to_owned()
        } else {
            units
                .iter()
                .map(|unit| unit.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        writeln!(
            output,
            "static const uint8_t {name}_data[{}] = {{ {payload} }};",
            units.len().max(1)
        )
        .unwrap();
        writeln!(
            output,
            "{linkage}const kiln_string {name} = {{ {}, KILN_STRING_LATIN1, {name}_data }};",
            units.len()
        )
        .unwrap();
    } else {
        let payload = if units.is_empty() {
            "0".to_owned()
        } else {
            units
                .iter()
                .map(|unit| format!("UINT16_C({unit})"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        writeln!(
            output,
            "static const uint16_t {name}_data[{}] = {{ {payload} }};",
            units.len().max(1)
        )
        .unwrap();
        writeln!(
            output,
            "{linkage}const kiln_string {name} = {{ {}, KILN_STRING_UTF16, {name}_data }};",
            units.len()
        )
        .unwrap();
    }
}

fn render_constant_declarations(
    output: &mut String,
    program: &Program,
    symbol: &str,
    compact: bool,
    should_chunk: bool,
) {
    for (index, constant) in program.constants.iter().enumerate() {
        match constant {
            Constant::String(value) => {
                let name = format!("{symbol}_string_{index}");
                render_static_string_with_linkage(output, &name, value, should_chunk);
                writeln!(output, "#define {symbol}_constant_{index} (&{name})").unwrap();
            }
            Constant::BigInt(digits) => {
                // Headed like every other string constant: the digits are ASCII, so the
                // static lands Latin1 and the keyed parse reads its payload in place.
                let name = format!("{symbol}_bigint_digits_{index}");
                render_static_string_with_linkage(
                    output,
                    &name,
                    &kiln_ir::StringUnits::from(digits.as_str()),
                    should_chunk,
                );
                writeln!(output, "#define {symbol}_constant_{index} (&{name})").unwrap();
                let linkage = if should_chunk { "" } else { "static " };
                writeln!(output, "{linkage}void *{symbol}_bigint_{index} = NULL;").unwrap();
            }
            Constant::Undefined
            | Constant::Null
            | Constant::Boolean(_)
            | Constant::Number(_)
            | Constant::WellKnownSymbol(_) => {}
        }
    }
    if compact {
        let mut unique_names: std::collections::BTreeMap<kiln_ir::StringUnits, usize> =
            std::collections::BTreeMap::new();
        let mut fn_ids = Vec::with_capacity(program.functions.len());
        for function in &program.functions {
            if let Some(name) = function.effective_name_units() {
                let next_id = unique_names.len();
                let id = *unique_names.entry(name).or_insert(next_id);
                fn_ids.push(Some(id));
            } else {
                fn_ids.push(None);
            }
        }
        for (name, id) in &unique_names {
            let symbol_name = format!("{symbol}_fn_name_{id}");
            render_static_string_with_linkage(output, &symbol_name, name, should_chunk);
        }
        for (index, id) in fn_ids.into_iter().enumerate() {
            if let Some(id) = id {
                writeln!(
                    output,
                    "#define {symbol}_function_name_{index} ({symbol}_fn_name_{id})"
                )
                .unwrap();
            }
        }
    } else {
        for (index, function) in program.functions.iter().enumerate() {
            if let Some(name) = function.effective_name_units() {
                let symbol_name = format!("{symbol}_function_name_{index}");
                render_static_string(output, &symbol_name, &name);
            }
        }
    }
}

fn render_constant_declarations_extern(
    output: &mut String,
    program: &Program,
    symbol: &str,
    compact: bool,
    include_function_names: bool,
) {
    for (index, constant) in program.constants.iter().enumerate() {
        match constant {
            Constant::String(_) => {
                let name = format!("{symbol}_string_{index}");
                writeln!(output, "extern const kiln_string {name};").unwrap();
                writeln!(output, "#define {symbol}_constant_{index} (&{name})").unwrap();
            }
            Constant::BigInt(_) => {
                let name = format!("{symbol}_bigint_digits_{index}");
                writeln!(output, "extern const kiln_string {name};").unwrap();
                writeln!(output, "#define {symbol}_constant_{index} (&{name})").unwrap();
                writeln!(output, "extern void *{symbol}_bigint_{index};").unwrap();
            }
            Constant::Undefined
            | Constant::Null
            | Constant::Boolean(_)
            | Constant::Number(_)
            | Constant::WellKnownSymbol(_) => {}
        }
    }
    if include_function_names {
        if compact {
            let mut unique_names: std::collections::BTreeMap<kiln_ir::StringUnits, usize> =
                std::collections::BTreeMap::new();
            let mut fn_ids = Vec::with_capacity(program.functions.len());
            for function in &program.functions {
                if let Some(name) = function.effective_name_units() {
                    let next_id = unique_names.len();
                    let id = *unique_names.entry(name).or_insert(next_id);
                    fn_ids.push(Some(id));
                } else {
                    fn_ids.push(None);
                }
            }
            for (_, id) in &unique_names {
                let symbol_name = format!("{symbol}_fn_name_{id}");
                writeln!(output, "extern const kiln_string {symbol_name};").unwrap();
            }
            for (index, id) in fn_ids.into_iter().enumerate() {
                if let Some(id) = id {
                    writeln!(
                        output,
                        "#define {symbol}_function_name_{index} ({symbol}_fn_name_{id})"
                    )
                    .unwrap();
                }
            }
        } else {
            for (index, function) in program.functions.iter().enumerate() {
                if function.effective_name_units().is_some() {
                    let symbol_name = format!("{symbol}_function_name_{index}");
                    writeln!(output, "extern const kiln_string {symbol_name};").unwrap();
                }
            }
        }
    }
}

fn render_regex_matchers(output: &mut String, program: &Program, symbol: &str, should_chunk: bool) {
    let mut seen = std::collections::BTreeSet::new();

    let emit_if_compiled = |inst: &Instruction,
                            seen: &mut std::collections::BTreeSet<(u32, u32)>,
                            output: &mut String| {
        if let Instruction::NewRegExp { pattern, flags } = inst
            && seen.insert((*pattern, *flags))
            && let (Constant::String(source), Constant::String(letters)) = (
                &program.constants[*pattern as usize],
                &program.constants[*flags as usize],
            )
            && let (Ok(source), Ok(letters)) =
                (source.to_scalar_string(), letters.to_scalar_string())
            && let Some(compiled) = crate::regex::try_compile_regex(&source, &letters)
        {
            let prefix = format!("{symbol}_re_{pattern}_{flags}");
            crate::regex::emit_regex_matcher(output, &prefix, &compiled, should_chunk);
        }
    };

    for inst in &program.code {
        emit_if_compiled(inst, &mut seen, output);
    }
    for func in &program.functions {
        for inst in &func.code {
            emit_if_compiled(inst, &mut seen, output);
        }
    }
}

fn render_regex_matchers_extern(output: &mut String, program: &Program, symbol: &str) {
    let mut seen = std::collections::BTreeSet::new();

    let emit_if_compiled = |inst: &Instruction,
                            seen: &mut std::collections::BTreeSet<(u32, u32)>,
                            output: &mut String| {
        if let Instruction::NewRegExp { pattern, flags } = inst
            && seen.insert((*pattern, *flags))
            && let (Constant::String(source), Constant::String(letters)) = (
                &program.constants[*pattern as usize],
                &program.constants[*flags as usize],
            )
            && let (Ok(source), Ok(letters)) =
                (source.to_scalar_string(), letters.to_scalar_string())
            && let Some(_compiled) = crate::regex::try_compile_regex(&source, &letters)
        {
            let prefix = format!("{symbol}_re_{pattern}_{flags}");
            use std::fmt::Write as _;
            writeln!(
                output,
                "int {prefix}(const uint16_t *units, size_t count, size_t start, int64_t *ranges, size_t capacity, size_t *groups, int *matched);\n\
                 int {prefix}_ascii(const uint8_t *units, size_t count, size_t start, int64_t *ranges, size_t capacity, size_t *groups, int *matched);"
            )
            .unwrap();
        }
    };

    for inst in &program.code {
        emit_if_compiled(inst, &mut seen, output);
    }
    for func in &program.functions {
        for inst in &func.code {
            emit_if_compiled(inst, &mut seen, output);
        }
    }
}

/// The constants a named-property instruction uses as its key, sorted and deduplicated: the
/// unit's share of the realm's intern table (ADR 0047).
fn property_key_constants(program: &Program) -> Vec<u32> {
    fn key_of(instruction: &Instruction) -> Option<u32> {
        match instruction {
            Instruction::GetProperty(name)
            | Instruction::SetProperty(name)
            | Instruction::DefineOwnProperty(name)
            | Instruction::DeleteProperty(name)
            | Instruction::LoadSuperProperty(name)
            | Instruction::SetPropertyStrict { name, .. }
            | Instruction::SetSuperPropertyStrict { name, .. }
            | Instruction::DeletePropertyStrict { name, .. }
            | Instruction::CallMethod { name, .. }
            | Instruction::CallMethodPrepared { name, .. } => Some(*name),
            _ => None,
        }
    }
    let mut keys: Vec<u32> = program
        .code
        .iter()
        .chain(program.functions.iter().flat_map(|proto| proto.code.iter()))
        .filter_map(key_of)
        .collect();
    literal_key_constants(program, &program.code, &mut keys);
    for proto in &program.functions {
        literal_key_constants(program, &proto.code, &mut keys);
    }
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// The string constants an object literal takes as its static keys -- under `NewObject`, or one
/// `DefineNamedProperty` at a time -- added to `keys`.
///
/// They join the unit's key table so the load-time constructor registers them as shared keys
/// (ADR 0071): a literal built from shared keys gets a shared shape, and a shared shape is the
/// only kind a literal site may remember and hand to objects of any realm
/// (`KILN_C99_SHAPE_IS_PORTABLE`). A literal whose keys stayed realm-local would be built
/// member by member every time.
///
/// The keys are found by walking the bytecode's stack like the validator does, labelling each slot
/// with the constant that pushed it, and reading the key positions under each `NewObject`. The
/// labels are exact for a key -- the compiler pushes a static key as `Const` directly beneath its
/// value -- and approximate above it, where only values live; a mislabel can only add a constant
/// to the table (a harmless registration) or miss one (a literal that is not cached), never make
/// a site wrong.
fn literal_key_constants(program: &Program, code: &[Instruction], keys: &mut Vec<u32>) {
    let mut seen = vec![false; code.len()];
    let mut work: Vec<(usize, Vec<Option<u32>>)> = vec![(0, Vec::new())];
    while let Some((mut index, mut labels)) = work.pop() {
        while index < code.len() && !seen[index] {
            seen[index] = true;
            let instruction = &code[index];
            let Ok((next_height, terminal)) =
                crate::validation::stack_effect(instruction, labels.len(), index)
            else {
                break;
            };
            match instruction {
                Instruction::Const(constant) => {
                    labels.push(Some(*constant));
                }
                Instruction::NewObject { count } => {
                    let count = *count as usize;
                    if labels.len() >= count * 2 {
                        let base = labels.len() - count * 2;
                        for pair in 0..count {
                            if let Some(constant) = labels[base + pair * 2]
                                && matches!(
                                    program.constants.get(constant as usize),
                                    Some(Constant::String(_))
                                )
                            {
                                keys.push(constant);
                            }
                        }
                    }
                    labels.resize(next_height, None);
                }
                // `{ ...rest, name: value }`: a member after a spread (or beside a computed key)
                // is defined one at a time, its static key the constant beneath its value.
                Instruction::DefineNamedProperty { .. } => {
                    if let Some(Some(constant)) = labels.len().checked_sub(2).map(|at| labels[at])
                        && matches!(
                            program.constants.get(constant as usize),
                            Some(Constant::String(_))
                        )
                    {
                        keys.push(constant);
                    }
                    labels.resize(next_height, None);
                }
                _ => labels.resize(next_height, None),
            }
            if terminal {
                break;
            }
            match instruction {
                Instruction::PushHandler(target) => {
                    let mut handler = labels.clone();
                    handler.push(None);
                    work.push((*target as usize, handler));
                }
                Instruction::Jump(target) => {
                    work.push((*target as usize, labels));
                    break;
                }
                Instruction::JumpIfFalse(target)
                | Instruction::JumpIfTruthy(target)
                | Instruction::JumpIfNotNullish(target)
                | Instruction::YieldStar(target)
                | Instruction::JumpIfReturnSignal(target) => {
                    work.push((*target as usize, labels.clone()));
                }
                _ => {}
            }
            index += 1;
        }
    }
}

/// One canonical pointer per key the unit names. A load-time constructor registers the unit's
/// constants into the process's shared key table before the first realm exists (ADR 0071) and
/// fills the pointers from it once, for every realm; `{symbol}_keys_shared` records that. When
/// registration could not vouch for every key -- a library loaded after a heap existed, or out of
/// memory -- the pointers are that of one realm at a time: `{symbol}_keys_owner` is the realm that
/// filled them at its entry (`kiln_c99_unit_keys_claim`), which gives them back at its
/// `KILN_GC_PHASE_RESET`, and a second live realm entering the unit meanwhile is refused rather
/// than handed the first one's keys.
///
/// The constructor names only this unit's own constants, so registration adds nothing to the
/// unit's cache key that its content did not already decide, and no launcher, embedding host or
/// shared-library entry point has to enumerate units: the linker's list of objects is the list.
fn render_key_table(
    output: &mut String,
    program: &Program,
    symbol: &str,
    keys: &[u32],
    should_chunk: bool,
) {
    if keys.is_empty() {
        return;
    }
    let linkage = if should_chunk { "" } else { "static " };
    if symbol == "ku" {
        writeln!(
            output,
            "{linkage}const kiln_string *ku_keys[{}];",
            keys.len()
        )
        .unwrap();
        for (i, index) in keys.iter().enumerate() {
            writeln!(output, "#define ku_key_{index} (ku_keys[{i}])").unwrap();
        }
    } else {
        for index in keys {
            writeln!(output, "{linkage}const kiln_string *{symbol}_key_{index};").unwrap();
        }
    }
    writeln!(output, "static uint64_t {symbol}_keys_owner;").unwrap();
    writeln!(output, "static int {symbol}_keys_shared;").unwrap();
    writeln!(
        output,
        "static const kiln_string *const {symbol}_key_sources[{}] = {{",
        keys.len()
    )
    .unwrap();
    for index in keys {
        let Constant::String(_) = &program.constants[*index as usize] else {
            unreachable!("verification rejects a non-string property name before emission")
        };
        writeln!(output, "    &{symbol}_string_{index},").unwrap();
    }
    writeln!(output, "}};").unwrap();
    writeln!(output, "static int {symbol}_intern_keys(void) {{").unwrap();
    writeln!(
        output,
        "    int claim = kiln_c99_unit_keys_claim(&{symbol}_keys_owner);
    if (claim > 0) return KILN_C99_OK;
    if (claim < 0) return kiln_c99_refuse(\"a module loaded after the first realm started running in a second live realm at once\");"
    )
    .unwrap();
    if symbol == "ku" && keys.len() > 4 {
        writeln!(
            output,
            "    for (size_t i = 0; i < (size_t){}; ++i) {{",
            keys.len()
        )
        .unwrap();
        writeln!(
            output,
            "        if ((ku_keys[i] = kiln_c99_intern_static_key(ku_key_sources[i])) == NULL) return KILN_C99_OUT_OF_MEMORY;"
        )
        .unwrap();
        writeln!(output, "    }}").unwrap();
    } else {
        for (i, index) in keys.iter().enumerate() {
            // The key store is canonical headed keys now: intern the constant's header
            // directly, in either width. The Latin1 bridge and its UTF-16 refusal are gone.
            if symbol == "ku" {
                writeln!(
                    output,
                    "    if ((ku_keys[{i}] = kiln_c99_intern_static_key(&{symbol}_string_{index})) == NULL) return KILN_C99_OUT_OF_MEMORY;"
                )
                .unwrap();
            } else {
                writeln!(
                    output,
                    "    if (({symbol}_key_{index} = kiln_c99_intern_static_key(&{symbol}_string_{index})) == NULL) return KILN_C99_OUT_OF_MEMORY;"
                )
                .unwrap();
            }
        }
    }
    writeln!(output, "    return KILN_C99_OK;\n}}").unwrap();
    // Registration. `ku` registers straight into its key array; any other symbol's keys are
    // separate statics, filled from a local array only once every key is known to be shared.
    writeln!(
        output,
        "__attribute__((constructor)) static void {symbol}_register_keys(void) {{"
    )
    .unwrap();
    if symbol == "ku" {
        writeln!(
            output,
            "    if (!kiln_c99_register_static_keys(ku_key_sources, ku_keys, (size_t){})) return;",
            keys.len()
        )
        .unwrap();
    } else {
        writeln!(output, "    const kiln_string *shared[{}];", keys.len()).unwrap();
        writeln!(
            output,
            "    if (!kiln_c99_register_static_keys({symbol}_key_sources, shared, (size_t){})) return;",
            keys.len()
        )
        .unwrap();
        for (i, index) in keys.iter().enumerate() {
            writeln!(output, "    {symbol}_key_{index} = shared[{i}];").unwrap();
        }
    }
    writeln!(output, "    {symbol}_keys_shared = 1;\n}}").unwrap();
}

/// The runtime bootstrap's string constants that read as names, registered as shared keys before
/// the first realm (realms issue 08) beside the property names `render_key_table` registers.
///
/// The bootstrap spells some of the names it defines as values -- `const added = ["count",
/// "countReset", ...]` then `Object.defineProperty(console, added[i], ...)` -- and a name that
/// reaches a property as a value is interned at run time, into the realm's own table, with a
/// realm-local shape for every object built with it: every realm paid for its own copy of the
/// same console and `DisposableStack` layouts. Registering the spelling first makes the run-time
/// intern find the shared key. Only a unit emitted with `shared_region` does this: the bootstrap's
/// strings are the built-ins' names; a program's are its data, which ADR 0071 keeps realm-local.
fn render_name_registration(output: &mut String, program: &Program, symbol: &str, keys: &[u32]) {
    let names: Vec<usize> = program
        .constants
        .iter()
        .enumerate()
        .filter(|(index, constant)| {
            let Constant::String(value) = constant else {
                return false;
            };
            if keys.contains(&(*index as u32)) {
                return false;
            }
            let units: Vec<u16> = value.units().collect();
            !units.is_empty()
                && units.len() <= 48
                && units.iter().enumerate().all(|(at, unit)| {
                    let byte = *unit as u8;
                    *unit < 0x80
                        && (byte.is_ascii_alphabetic()
                            || byte == b'_'
                            || byte == b'$'
                            || (at > 0 && byte.is_ascii_digit()))
                })
        })
        .map(|(index, _)| index)
        .collect();
    if names.is_empty() {
        return;
    }
    writeln!(
        output,
        "static const kiln_string *const {symbol}_name_sources[{}] = {{",
        names.len()
    )
    .unwrap();
    for index in &names {
        writeln!(output, "    &{symbol}_string_{index},").unwrap();
    }
    writeln!(output, "}};").unwrap();
    writeln!(
        output,
        "__attribute__((constructor)) static void {symbol}_register_names(void) {{\n    (void)kiln_c99_register_static_keys({symbol}_name_sources, NULL, (size_t){});\n}}",
        names.len()
    )
    .unwrap();
}

fn render_key_table_extern(output: &mut String, symbol: &str, keys: &[u32]) {
    if keys.is_empty() {
        return;
    }
    if symbol == "ku" {
        writeln!(output, "extern const kiln_string *ku_keys[];").unwrap();
        for (i, index) in keys.iter().enumerate() {
            writeln!(output, "#define ku_key_{index} (ku_keys[{i}])").unwrap();
        }
    } else {
        for index in keys {
            writeln!(output, "extern const kiln_string *{symbol}_key_{index};").unwrap();
        }
    }
}

/// The line a unit's entry point runs to canonicalize its keys before the first access. A key
/// that cannot be interned fails the entry: no site ever runs with a pointer that is not
/// canonical.
fn render_key_prologue(output: &mut String, symbol: &str, keyed: bool) {
    if keyed {
        writeln!(
            output,
            "    if (!{symbol}_keys_shared) {{ int keyed = {symbol}_intern_keys(); if (keyed != KILN_C99_OK) return keyed; }}"
        )
        .unwrap();
    }
}

/// The root function this unit registers with the realm (ADR 0041): its module-scope slots,
/// when it has them, and the prelude statics every unit owns a copy of.
fn render_template_sites(output: &mut String, program: &Program, symbol: &str, should_chunk: bool) {
    if program.template_sites.is_empty() {
        return;
    }
    for (site_idx, site) in program.template_sites.iter().enumerate() {
        for (raw_idx, raw) in site.raw.iter().enumerate() {
            let name = format!("{symbol}_site_{site_idx}_raw_{raw_idx}");
            render_static_string_with_linkage(output, &name, raw, should_chunk);
        }
        for (cooked_idx, cooked) in site.cooked.iter().enumerate() {
            if let Some(text) = cooked {
                let name = format!("{symbol}_site_{site_idx}_cooked_{cooked_idx}");
                render_static_string_with_linkage(output, &name, text, should_chunk);
            }
        }
    }
    let linkage = if should_chunk { "" } else { "static " };
    writeln!(
        output,
        "{linkage}int {symbol}_materialize_template_site(kiln_host *host, size_t site) {{"
    )
    .unwrap();
    writeln!(output, "    switch (site) {{").unwrap();
    for (site_idx, site) in program.template_sites.iter().enumerate() {
        let raw_len = site.raw.len();
        let cooked_len = site.cooked.len();
        writeln!(output, "    case {site_idx}: {{").unwrap();
        writeln!(output, "        kiln_value raw_built;").unwrap();
        writeln!(
            output,
            "        int status = kiln_c99_new_array_with_capacity(&raw_built, {raw_len}, 0);"
        )
        .unwrap();
        writeln!(output, "        if (status != KILN_C99_OK) return status;").unwrap();
        writeln!(
            output,
            "        kiln_array *raw_arr = kiln_c99_array_of(raw_built);"
        )
        .unwrap();
        for raw_idx in 0..raw_len {
            writeln!(
                output,
                "        kiln_c99_heap_store_val(raw_arr, &raw_arr->elements[{raw_idx}], KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &{symbol}_site_{site_idx}_raw_{raw_idx}));"
            )
            .unwrap();
        }
        writeln!(output, "        raw_arr->count = {raw_len};").unwrap();
        writeln!(output, "        raw_arr->frozen = 1;").unwrap();

        writeln!(output, "        kiln_value cooked_built;").unwrap();
        writeln!(
            output,
            "        status = kiln_c99_new_array_with_capacity(&cooked_built, {cooked_len}, 0);"
        )
        .unwrap();
        writeln!(output, "        if (status != KILN_C99_OK) return status;").unwrap();
        writeln!(
            output,
            "        kiln_array *cooked_arr = kiln_c99_array_of(cooked_built);"
        )
        .unwrap();
        for (cooked_idx, cooked) in site.cooked.iter().enumerate() {
            if cooked.is_some() {
                writeln!(
                    output,
                    "        kiln_c99_heap_store_val(cooked_arr, &cooked_arr->elements[{cooked_idx}], KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &{symbol}_site_{site_idx}_cooked_{cooked_idx}));"
                )
                .unwrap();
            } else {
                writeln!(
                    output,
                    "        kiln_c99_heap_store_val(cooked_arr, &cooked_arr->elements[{cooked_idx}], KILN_VAL_UNDEFINED);"
                )
                .unwrap();
            }
        }
        writeln!(output, "        cooked_arr->count = {cooked_len};").unwrap();

        writeln!(output, "        kiln_value props;").unwrap();
        writeln!(output, "        status = kiln_c99_new_object(&props);").unwrap();
        writeln!(output, "        if (status != KILN_C99_OK) return status;").unwrap();
        writeln!(output, "        kiln_property *raw_slot;").unwrap();
        writeln!(
            output,
            "        status = kiln_c99_own_slot(props, \"raw\", 3, &raw_slot);"
        )
        .unwrap();
        writeln!(output, "        if (status != KILN_C99_OK) return status;").unwrap();
        writeln!(output, "        kiln_c99_heap_store_value(kiln_c99_object_of(props), &raw_slot->value, raw_built);").unwrap();
        writeln!(output, "        raw_slot->writable = 0;").unwrap();
        writeln!(output, "        raw_slot->enumerable = 0;").unwrap();
        writeln!(output, "        raw_slot->configurable = 0;").unwrap();
        writeln!(output, "        raw_slot->accessor = 0;").unwrap();
        writeln!(
            output,
            "        kiln_c99_object_of(props)->read_only_seen = 1;"
        )
        .unwrap();
        writeln!(
            output,
            "        kiln_c99_heap_store_value(cooked_arr, &cooked_arr->properties, props);"
        )
        .unwrap();
        writeln!(output, "        cooked_arr->frozen = 1;").unwrap();

        writeln!(
            output,
            "        {symbol}_template_sites[{site_idx}] = kiln_val_from_boundary(cooked_built);"
        )
        .unwrap();
        writeln!(output, "        return KILN_C99_OK;").unwrap();
        writeln!(output, "    }}").unwrap();
    }
    writeln!(output, "    default: return KILN_C99_CONTROL_ERROR;").unwrap();
    writeln!(output, "    }}").unwrap();
    writeln!(output, "}}").unwrap();
}

/// How many module globals a unit keeps in its realm block (ADR 0070, realms issue 04 step 5).
fn unit_global_count(program: &Program) -> usize {
    if uses_module_scope(program) {
        usize::try_from(program.local_count).unwrap().max(1)
    } else {
        0
    }
}

/// The unit's realm state: its module globals, then its template sites, in one block each realm
/// allocates on its first entry into the unit, marks through its heap and frees with it (see
/// `kiln_c99_unit` in `prelude_types.h`). `{symbol}_globals` and `{symbol}_template_sites` name
/// this realm's block wherever they are written, through a per-thread cache checked against the
/// realm generation; a function body that reads its module's globals loads the pointer once at
/// entry instead (`emit_graph::body`).
///
/// `unit_linkage` is the descriptor's: external in a chunked unit, whose chunks name it, which is
/// also where `extern_only` renders a chunk's view of it. Each translation unit keeps its own
/// cache; the block is the realm's either way.
fn render_unit_state(
    output: &mut String,
    symbol: &str,
    globals: usize,
    template_sites: usize,
    compact_cells: usize,
    should_chunk: bool,
    extern_only: bool,
    external: bool,
) {
    if globals + template_sites + compact_cells == 0 {
        return;
    }
    if extern_only {
        writeln!(output, "extern kiln_c99_unit {symbol}_unit;").unwrap();
    } else {
        let linkage = if should_chunk || external {
            ""
        } else {
            "static "
        };
        writeln!(
            output,
            "{linkage}kiln_c99_unit {symbol}_unit = {{ 0, {} }};",
            globals + template_sites + compact_cells
        )
        .unwrap();
    }
    writeln!(
        output,
        "static _Thread_local kiln_c99_unit_cache {symbol}_unit_cached;\n\
         static inline __attribute__((unused)) kiln_val *{symbol}_unit_vals(kiln_host *host) {{ return kiln_c99_unit_vals_for_host(host, &{symbol}_unit, &{symbol}_unit_cached); }}"
    )
    .unwrap();
    if globals > 0 {
        writeln!(
            output,
            "#define {symbol}_globals ({symbol}_unit_vals(host))"
        )
        .unwrap();
    }
    if template_sites > 0 {
        writeln!(
            output,
            "#define {symbol}_template_sites ({symbol}_unit_vals(host) + {globals})"
        )
        .unwrap();
    }
    if compact_cells > 0 {
        writeln!(
            output,
            "#define {symbol}_cell_slots ({symbol}_unit_vals(host) + {})",
            globals + template_sites
        )
        .unwrap();
    }
}

fn render_unit_roots(output: &mut String, symbol: &str, keyed: bool) {
    writeln!(
        output,
        "static void {symbol}_gc_roots(void *heap, void *context, int phase) {{"
    )
    .unwrap();
    if keyed {
        // Unless registration made them shared, the canonical pointers belong to the owning
        // realm's intern table, which dies with the realm.
        writeln!(
            output,
            "    if (phase == KILN_GC_PHASE_RESET && !{symbol}_keys_shared) kiln_c99_unit_keys_release(&{symbol}_keys_owner, heap);"
        )
        .unwrap();
    }
    // Module globals and template sites are in the realm's block, which its heap marks.
    output.push_str("    kiln_c99_gc_unit_roots(heap, context, phase);\n}\n");
    writeln!(
        output,
        "static int {symbol}_gc_visit_roots(void *context, kiln_gc_visit_root_fn visit, void *visit_context) {{\n    const kiln_value *roots[] = {{ &kiln_c99_new_target, &kiln_c99_adopted_this, &kiln_c99_captured_arguments, &kiln_c99_frame_arguments_object, &kiln_c99_frame_callee }};\n    size_t index;\n    (void)context;\n    for (index = 0; index < sizeof roots / sizeof roots[0]; ++index)\n        if (visit(visit_context, roots[index], KILN_GC_VISIT_VALUE) != 0) return KILN_C99_HOST_ERROR;\n    return KILN_C99_OK;\n}}"
    )
    .unwrap();
}

fn render_scalar_realm_wrapper(output: &mut String, symbol: &str) {
    writeln!(
        output,
        r#"
typedef struct {symbol}_realm_context {{
    kiln_host *host;
    kiln_value *result;
}} {symbol}_realm_context;

static int {symbol}_gc_visit_result(void *context, kiln_gc_visit_root_fn visit,
                                     void *visit_context) {{
    if (context == NULL) return KILN_C99_OK;
    return visit(visit_context, context, KILN_GC_VISIT_VALUE);
}}

static int {symbol}_gc_result_entry(void *result) {{
    return kiln_realm_gc_register_roots_with_visitor(kiln_c99_gc_result_roots,
             {symbol}_gc_visit_result, result, KILN_GC_ROOT_SURVIVES_TEARDOWN);
}}

static int {symbol}_realm_entry(void *opaque) {{
    {symbol}_realm_context *context = ({symbol}_realm_context *)opaque;
    if (context == NULL) return KILN_C99_INVALID_ARGUMENT;
    return {symbol}_body(context->host, context->result);
}}

int {symbol}(kiln_host *host, kiln_value *result) {{
    {symbol}_realm_context context;
    uint64_t realm = 0;
    int status;
    int destroy_status;
    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (kiln_realm_is_active()) return {symbol}_body(host, result);
    context.host = host;
    context.result = result;
    status = kiln_realm_create(&realm);
    if (status == KILN_C99_OK)
        status = kiln_realm_enter(realm, {symbol}_gc_result_entry, result);
    if (status == KILN_C99_OK)
        status = kiln_realm_enter(realm, {symbol}_realm_entry, &context);
    if (status == KILN_C99_OK)
        status = kiln_realm_drive(realm, host, result);
    if (realm != 0) {{
        destroy_status = kiln_realm_destroy(realm);
        if (status == KILN_C99_OK) status = destroy_status;
    }}
    if (status != KILN_C99_OK && kiln_c99_crosses_boundary(*result))
        *result = kiln_c99_text(*result);
    return status;
}}
"#
    )
    .unwrap();
}

fn render_module_realm_wrapper(
    output: &mut String,
    symbol: &str,
    unit_symbol: &str,
    commonjs_site: Option<&str>,
) {
    let invocation = if let Some(site) = commonjs_site {
        // CommonJS evaluation is a real wrapper activation. It enters the module body directly,
        // rather than calling a closure, so publish its frame here and restore it on every return.
        // Otherwise captureStackTrace loses the caller of module-level depd initialization.
        let source = c_string(site);
        writeln!(
            output,
            r#"
static int {unit_symbol}_invoke_body(kiln_host *host, kiln_value *result,
    const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count) {{
    kiln_js_frame module_frame;
    kiln_c99_js_frame_enter(&module_frame, NULL);
    kiln_c99_js_frame_note_instruction({source}, 0, 0, 0);
    int status = {unit_symbol}_body(host, result, imports, import_count, exports, export_count);
    kiln_c99_js_frame_leave(&module_frame);
    return status;
}}
"#
        )
        .unwrap();
        format!("{unit_symbol}_invoke_body")
    } else {
        format!("{unit_symbol}_body")
    };
    writeln!(
        output,
        r#"
typedef struct {unit_symbol}_realm_context {{
    kiln_host *host;
    kiln_value *result;
    const kiln_value *imports;
    size_t import_count;
    kiln_value *exports;
    size_t export_count;
}} {unit_symbol}_realm_context;

static int {unit_symbol}_gc_visit_result(void *context, kiln_gc_visit_root_fn visit,
                                         void *visit_context) {{
    if (context == NULL) return KILN_C99_OK;
    return visit(visit_context, context, KILN_GC_VISIT_VALUE);
}}

static int {unit_symbol}_gc_result_entry(void *result) {{
    return kiln_realm_gc_register_roots_with_visitor(kiln_c99_gc_result_roots,
             {unit_symbol}_gc_visit_result, result, KILN_GC_ROOT_SURVIVES_TEARDOWN);
}}

static int {unit_symbol}_realm_entry(void *opaque) {{
    {unit_symbol}_realm_context *context = ({unit_symbol}_realm_context *)opaque;
    int status;
    if (context == NULL) return KILN_C99_INVALID_ARGUMENT;
    status = {invocation}(context->host, context->result, context->imports,
                           context->import_count, context->exports, context->export_count);
    /* A graph is linked from one C translation unit per JavaScript module. Diagnostic state is
     * deliberately private to each unit, so the module that owns the refusing instruction must
     * materialize it before control returns to a caller whose globals describe a different
     * instruction. The result value is the graph boundary's existing error channel. */
    if (status == KILN_C99_CONTROL_ERROR)
        *context->result = kiln_c99_control_diagnostic();
    return status;
}}

int {symbol}(kiln_host *host, kiln_value *result, const kiln_value *imports,
             size_t import_count, kiln_value *exports, size_t export_count) {{
    {unit_symbol}_realm_context context;
    uint64_t realm = 0;
    int status;
    int destroy_status;
    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (kiln_realm_is_active()) {{
        (void)kiln_realm_diagnostic_reset();
        status = {invocation}(host, result, imports, import_count, exports, export_count);
        if (status == KILN_C99_CONTROL_ERROR)
            *result = kiln_c99_control_diagnostic();
        return status;
    }}
    kiln_c99_diagnostic_site = NULL;
    kiln_c99_diagnostic_operation = NULL;
    kiln_c99_diagnostic_instruction = 0;
    kiln_c99_diagnostic_depth = 0;
    kiln_c99_diagnostic_method = 0;
    kiln_c99_diagnostic_receiver_tag = 0;
    kiln_c99_diagnostic_has_method = 0;
    kiln_c99_diagnostic_detail = NULL;
    kiln_c99_diagnostic_function = NULL;
    kiln_c99_diagnostic_function_len = 0;
    kiln_c99_diagnostic_function_key = NULL;
    context.host = host;
    context.result = result;
    context.imports = imports;
    context.import_count = import_count;
    context.exports = exports;
    context.export_count = export_count;
    status = kiln_realm_create(&realm);
    if (status == KILN_C99_OK)
        status = kiln_realm_enter(realm, {unit_symbol}_gc_result_entry, result);
    if (status == KILN_C99_OK)
        status = kiln_realm_enter(realm, {unit_symbol}_realm_entry, &context);
    if (status == KILN_C99_OK)
        status = kiln_realm_drive(realm, host, result);
    if (realm != 0) {{
        destroy_status = kiln_realm_destroy(realm);
        if (status == KILN_C99_OK) status = destroy_status;
    }}
    if (status != KILN_C99_OK && kiln_c99_crosses_boundary(*result))
        *result = kiln_c99_text(*result);
    return status;
}}
"#
    )
    .unwrap();
}

/// Emit the private object-layout seam used by native graph launchers to materialize ESM
/// namespace snapshots. The launcher owns the graph's export values but not this translation
/// unit's object representation, so construction stays here beside `kiln_c99_new_object`.
fn render_namespace_wrapper(output: &mut String, symbol: &str) {
    writeln!(
        output,
        r#"
int {symbol}_namespace_wrapper(kiln_value *result, const char *const *names,
                               const size_t *name_lengths, const kiln_value *values,
                               size_t count) {{
    kiln_value object;
    kiln_object *data = NULL;
    size_t *order = NULL;
    size_t index;
    size_t insert_at;
    int status = KILN_C99_OK;
    int extending = 0;
    if (result == NULL || (count != 0 && (names == NULL || name_lengths == NULL || values == NULL)))
        return KILN_C99_INVALID_ARGUMENT;
    for (index = 0; index < count; ++index) {{
        if (names[index] == NULL) return KILN_C99_INVALID_ARGUMENT;
        /* The current generated runtime uses @@-prefixed strings for symbol-keyed protocol
         * slots. Refuse an ambiguous export at this private seam rather than aliasing it with
         * Symbol.toStringTag and silently returning the wrong namespace. */
        if (name_lengths[index] >= 2 && names[index][0] == '@' && names[index][1] == '@')
            return KILN_C99_INVALID_ARGUMENT;
        if (index != 0 && name_lengths[index - 1] == name_lengths[index] &&
            memcmp(names[index - 1], names[index], name_lengths[index]) == 0)
            return KILN_C99_INVALID_ARGUMENT;
    }}
    if (count != 0) {{
        if (count > SIZE_MAX / sizeof *order) return KILN_C99_OUT_OF_MEMORY;
        order = (size_t *)malloc(count * sizeof *order);
        if (order == NULL) return KILN_C99_OUT_OF_MEMORY;
        for (index = 0; index < count; ++index) order[index] = index;
        /* Stable insertion sort keeps equal source inputs deterministic without relying on a
         * qsort callback that cannot carry the caller's parallel arrays. */
        for (index = 1; index < count; ++index) {{
            size_t at = index;
            size_t candidate = order[index];
            while (at != 0) {{
                size_t previous = order[at - 1];
                size_t common = name_lengths[previous] < name_lengths[candidate]
                                    ? name_lengths[previous] : name_lengths[candidate];
                int comparison = common == 0 ? 0 : memcmp(names[previous], names[candidate], common);
                if (comparison < 0 ||
                    (comparison == 0 && name_lengths[previous] <= name_lengths[candidate]))
                    break;
                order[at] = previous;
                --at;
            }}
            order[at] = candidate;
        }}
        for (index = 1; index < count; ++index) {{
            size_t previous = order[index - 1];
            size_t current = order[index];
            if (name_lengths[previous] == name_lengths[current] &&
                (name_lengths[current] == 0 ||
                 memcmp(names[previous], names[current], name_lengths[current]) == 0)) {{
                free(order);
                return KILN_C99_INVALID_ARGUMENT;
            }}
        }}
    }}
    if (result->tag == KILN_VALUE_OBJECT &&
        kiln_c99_object_of(*result)->module_namespace) {{
        object = *result;
        data = kiln_c99_object_of(object);
        extending = 1;
    }} else {{
        status = kiln_c99_new_object(&object);
        if (status != KILN_C99_OK) {{
            free(order);
            return status;
        }}
        data = kiln_c99_object_of(object);
        data->shape = NULL;
        kiln_c99_heap_store_value(data, &data->prototype, kiln_c99_null());
    }}
    for (index = 0; index < count; ++index) {{
        size_t source = order[index];
        kiln_property *slot;
        kiln_value value = values[source];
        const kiln_string *key = kiln_c99_intern(names[source], name_lengths[source]);
        size_t existing;
        if (key == NULL) {{
            status = KILN_C99_OUT_OF_MEMORY;
            goto namespace_failure;
        }}
        /* An instantiated namespace is first created empty, then filled with its export cells.
         * After evaluation the launcher publishes the same export names again. Treat that as a
         * value refresh, not a second property insertion: namespace [[OwnPropertyKeys]] is
         * unique, and existing export descriptors remain writable/non-configurable. */
        for (existing = 0; existing < data->count; ++existing) {{
            if (data->properties[existing].key == key) {{
                kiln_c99_heap_store_value(data, &data->properties[existing].value, value);
                goto namespace_entry_done;
            }}
        }}
        status = kiln_c99_reserve((void **)&data->properties, &data->capacity, data->count + 1,
                                  sizeof *data->properties);
        if (status != KILN_C99_OK) {{
            goto namespace_failure;
        }}
        /* Namespace export keys stay in the linker's lexical order. Ordinary object insertion
         * would move integer-looking names to numeric order, which is not the namespace exotic
         * object's [[OwnPropertyKeys]] rule. */
        insert_at = data->count;
        if (extending && data->count != 0 &&
            kiln_string_equals_ascii(data->properties[data->count - 1].key,
                                     "@@toStringTag", 13)) {{
            insert_at = data->count - 1;
            memmove(&data->properties[insert_at + 1], &data->properties[insert_at],
                    (data->count - insert_at) * sizeof *data->properties);
        }}
        slot = &data->properties[insert_at];
        /* Canonical like every stored key (ADR 0047). Copied rather than registered as static:
         * the launcher's names are its own, and this seam promises nothing about their life. */
        slot->key = key;
        data->count += 1;
        kiln_c99_heap_store_value(data, &slot->value, value);
        slot->symbol_hidden = 0;
        slot->writable = 1;
        slot->enumerable = 1;
        slot->configurable = 0;
        slot->accessor = 0;
        slot->refused = 0;
namespace_entry_done:;
    }}
    if (!extending) {{
        kiln_property *tag;
        status = kiln_c99_own_slot(object, "@@toStringTag", 13, &tag);
        if (status == KILN_C99_OK) {{
            kiln_c99_heap_store_value(data, &tag->value, kiln_c99_copy_text("Module", 6));
            if (tag->value.tag != KILN_VALUE_STRING) status = KILN_C99_OUT_OF_MEMORY;
            tag->writable = 0;
            kiln_c99_object_of(object)->read_only_seen = 1;
            tag->enumerable = 0;
            tag->symbol_hidden = 1;
            tag->configurable = 0;
            tag->accessor = 0;
        }}
    }}
    free(order);
    if (status != KILN_C99_OK) goto namespace_failure_without_order;
    data->module_namespace = 1;
    *result = object;
    return KILN_C99_OK;

namespace_failure:
    free(order);
namespace_failure_without_order:
    if (data != NULL && !extending) {{
        for (index = 0; index < data->count; ++index) {{
            if (data->properties[index].key != NULL &&
                kiln_string_equals_ascii(data->properties[index].key, "@@toStringTag", 13) &&
                data->properties[index].value.tag == KILN_VALUE_STRING)
                kiln_c99_gc_release((void *)data->properties[index].value.ptr);
        }}
        /* The object is a collected block; releasing it runs the finaliser that owns
         * `properties`, and unlinks it from the heap the way free() could not. */
        kiln_c99_gc_release(data);
    }}
    return status;
}}

"#
    )
    .unwrap();
}

fn render_emission_policy(output: &mut String, policy: &EmissionPolicy) {
    if *policy == EmissionPolicy::default() {
        return;
    }
    writeln!(
        output,
        "#define KILN_REALM_HOST_CLOCK {}",
        u8::from(policy.host_clock)
    )
    .unwrap();
    if policy.no_kiln_extensions {
        output.push_str("#define KILN_REALM_NO_EXTENSIONS 1\n");
    }
    match policy.entropy {
        EmissionEntropyPolicy::Deterministic => {}
        EmissionEntropyPolicy::Seeded(seed) => {
            writeln!(output, "#define KILN_REALM_ENTROPY_SEED UINT64_C({seed})").unwrap();
        }
        EmissionEntropyPolicy::Host => {
            output.push_str("#define KILN_REALM_HOST_ENTROPY 1\n");
        }
    }
}

fn render_compact_macros(output: &mut String, program: &Program, closure_fn: &str) {
    let mut has_call = false;
    let mut has_call_method = false;
    let mut has_closure = false;
    let mut has_cell = false;
    let mut has_binary = false;
    let mut has_read_property = false;
    let mut has_write_property = false;
    let mut has_new_object = false;
    let mut has_status = false;
    let mut has_truthy = false;

    let check_inst = |inst: &Instruction,
                      has_status: &mut bool,
                      has_call: &mut bool,
                      has_call_method: &mut bool,
                      has_closure: &mut bool,
                      has_cell: &mut bool,
                      has_binary: &mut bool,
                      has_read_property: &mut bool,
                      has_write_property: &mut bool,
                      has_new_object: &mut bool,
                      has_truthy: &mut bool| {
        match inst {
            Instruction::NewObject { .. } => {
                *has_new_object = true;
                *has_status = true;
            }
            Instruction::Call { .. } => {
                *has_call = true;
                *has_status = true;
            }
            Instruction::CallMethod { .. } => {
                *has_call_method = true;
                *has_read_property = true;
                *has_status = true;
                *has_truthy = true;
            }
            Instruction::MakeFunction(_) => *has_closure = true,
            Instruction::LoadCell(_) | Instruction::LoadUpvalue(_) => {
                *has_cell = true;
                *has_status = true;
            }
            Instruction::Binary(_) => {
                *has_binary = true;
                *has_status = true;
            }
            Instruction::GetProperty(_) => {
                *has_read_property = true;
                *has_status = true;
            }
            Instruction::SetProperty(_)
            | Instruction::SetPropertyStrict { .. }
            | Instruction::DefineOwnProperty(_) => {
                *has_write_property = true;
                *has_status = true;
            }
            Instruction::JumpIfFalse(_)
            | Instruction::JumpIfTruthy(_)
            | Instruction::Unary(UnaryOp::Not) => {
                *has_truthy = true;
            }
            Instruction::SetComputed
            | Instruction::SetComputedStrict { .. }
            | Instruction::DefineOwnComputed
            | Instruction::SetPrototype
            | Instruction::SetSuperPropertyStrict { .. }
            | Instruction::SetSuperElementStrict { .. }
            | Instruction::SetPrivate(_)
            | Instruction::DefineNamedProperty { .. }
            | Instruction::Unary(_)
            | Instruction::DirectEval(_) => {
                *has_status = true;
            }
            _ => {}
        }
    };

    for inst in &program.code {
        check_inst(
            inst,
            &mut has_status,
            &mut has_call,
            &mut has_call_method,
            &mut has_closure,
            &mut has_cell,
            &mut has_binary,
            &mut has_read_property,
            &mut has_write_property,
            &mut has_new_object,
            &mut has_truthy,
        );
    }
    for func in &program.functions {
        for inst in &func.code {
            check_inst(
                inst,
                &mut has_status,
                &mut has_call,
                &mut has_call_method,
                &mut has_closure,
                &mut has_cell,
                &mut has_binary,
                &mut has_read_property,
                &mut has_write_property,
                &mut has_new_object,
                &mut has_truthy,
            );
        }
    }
    if has_truthy {
        output.push_str("static __attribute__((unused, noinline)) int ku_truthy_slow(kiln_val v) { return kiln_c99_val_truthy(v); }\nstatic inline int ku_truthy(kiln_val v) {\n    if (v == KILN_VAL_FALSE || v == KILN_VAL_UNDEFINED || v == KILN_VAL_NULL) return 0;\n    if (v == KILN_VAL_TRUE) return 1;\n    uint64_t tag = v & KILN_TAG_MASK;\n    if (tag >= KILN_TAG_OBJECT_PREFIX && tag <= KILN_TAG_SYMBOL_PREFIX) return 1;\n    return ku_truthy_slow(v);\n}\n#define kiln_c99_val_truthy ku_truthy\n");
    }
    // Unconditional: the instruction scan above decides the other macros, but every runtime
    // call the ops emit ends in `KU_STATUS`, and the scan is a hand list -- `New` was not on it,
    // so a promise executor whose only operation was `new TypeError(..)` referenced a macro the
    // unit never defined. An unused `#define` costs nothing; a missing one is a build failure
    // in the program that happens to hit the next instruction the list forgot.
    let _ = has_status;
    {
        output.push_str("#define KU_STATUS(PC, OP, UNWIND) do { if (KILN_UNLIKELY(status != KILN_C99_OK)) { if (status == KILN_C99_THROWN) { UNWIND; } if (status == KILN_C99_CONTROL_ERROR) { kiln_c99_note_refusal(ku_site, (size_t)(PC), (OP), (size_t)kiln_c99_diagnostic_active_depth); goto kiln_refusal; } return status; } } while (0)\n");
    }
    if has_cell {
        output.push_str("#define KU_LOAD_CELL(DST, CELL, THROWN, PC, OP, UNWIND) do { const kiln_val *ku_cell = (const kiln_val *)KILN_VAL_PTR(CELL); int status = kiln_c99_val_check_tdz(*ku_cell, (THROWN)); KU_STATUS((PC), (OP), UNWIND); (DST) = *ku_cell; } while (0)\n");
    }
    if has_binary {
        output.push_str("static __attribute__((unused)) inline kiln_val ku_binary_fast(uint16_t op, kiln_host *host, kiln_val left, kiln_val right, int *status, int depth, kiln_value *thrown) {\n\
    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(left) && KILN_VAL_IS_NUMBER(right))) return kiln_c99_val_binary_fast(op, left, right);\n\
    if (KILN_LIKELY((op == 1 || op == 7 || op == 8 || op == 15 || op == 16) && KILN_VAL_IS_STRING(left) && KILN_VAL_IS_STRING(right))) return kiln_c99_val_binary_fast(op, left, right);\n\
    { kiln_val immediate = KILN_VAL_EQUALITY_IMMEDIATE(op, left, right); if (immediate != KILN_VAL_HOLE) return immediate; }\n\
    return kiln_c99_val_binary(op, host, left, right, status, depth, thrown);\n\
}\n");
        output.push_str("#define KU_BINARY(DST, CODE, LEFT, RIGHT, PC, OP, UNWIND) do { int status = KILN_C99_OK; (DST) = ku_binary_fast((CODE), host, (LEFT), (RIGHT), &status, depth, thrown); KU_STATUS((PC), (OP), UNWIND); } while (0)\n");
    }
    if has_read_property {
        // The inline check reads the first entry only, so an entry that keeps answering here moves
        // to the front -- while no other thread runs a realm (`kiln_c99_site_answered`).
        output.push_str("static __attribute__((unused, noinline)) kiln_val ku_pic_poly_read(const kiln_object *obj, kiln_c99_site *site) {\n\
    const kiln_shape *shape = obj->shape;\n\
    uint32_t count = kiln_c99_site_count(site);\n\
    for (uint32_t i = 1; i < count; ++i) {\n\
        if (shape == kiln_c99_slot_key(&site->entries[i].shape)) {\n\
            uint32_t slot = site->entries[i].slot;\n\
            kiln_val answer;\n\
            if (KILN_LIKELY(slot < obj->slot_capacity)) answer = obj->slots[slot];\n\
            else if (!kiln_c99_pic_inherited(&site->entries[i], obj, &answer)) continue;\n\
            kiln_c99_site_answered(site, i);\n\
            return answer;\n\
        }\n\
    }\n\
    return KILN_VAL_HOLE;\n\
}\n");
        output.push_str("#define KU_READ_PROPERTY(DST, OBJECT, KEY, KEY_LEN, SITE, PC, OP, UNWIND) do { \\\n\
    kiln_c99_site *ku_s = (SITE); \\\n\
    kiln_val ku_t = (OBJECT); \\\n\
    if (KILN_LIKELY(ku_s != NULL && kiln_c99_site_count(ku_s) != 0 && KILN_VAL_IS_OBJECT(ku_t))) { \\\n\
        const kiln_object *ku_obj = (const kiln_object *)KILN_VAL_PTR(ku_t); \\\n\
        if (KILN_LIKELY(ku_obj != NULL && ku_obj->shape != NULL && ku_obj->shape == kiln_c99_slot_key(&ku_s->entries[0].shape))) { \\\n\
            uint32_t ku_s0 = ku_s->entries[0].slot; \\\n\
            if (KILN_LIKELY(ku_s0 < ku_obj->slot_capacity)) { \\\n\
                (DST) = ku_obj->slots[ku_s0]; \\\n\
                break; \\\n\
            } else if (kiln_c99_pic_inherited(&ku_s->entries[0], ku_obj, &(DST))) { \\\n\
                break; \\\n\
            } \\\n\
        } else if (ku_obj != NULL && kiln_c99_site_count(ku_s) > 1) { \\\n\
            kiln_val ku_pv = ku_pic_poly_read(ku_obj, ku_s); \\\n\
            if (ku_pv != KILN_VAL_HOLE) { (DST) = ku_pv; break; } \\\n\
        } \\\n\
    } else if (ku_s != NULL && kiln_c99_site_length_hint(ku_s) != 0) { \\\n\
        if (kiln_c99_site_length_hint(ku_s) == KILN_VALUE_ARRAY && ((ku_t & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) && kiln_c99_gc_payload_kind(KILN_VAL_PTR(ku_t)) == KILN_GC_ARRAY) { \\\n\
            const kiln_array *ku_arr = (const kiln_array *)KILN_VAL_PTR(ku_t); \\\n\
            (DST) = kiln_c99_val_from_double((double)ku_arr->count); \\\n\
            break; \\\n\
        } else if (kiln_c99_site_length_hint(ku_s) == KILN_VALUE_STRING && KILN_VAL_IS_STRING(ku_t)) { \\\n\
            const kiln_string *ku_str = (const kiln_string *)KILN_VAL_PTR(ku_t); \\\n\
            (DST) = kiln_c99_val_from_double((double)(ku_str != NULL ? ku_str->unit_len : 0)); \\\n\
            break; \\\n\
        } \\\n\
    } \\\n\
    int status = kiln_c99_val_read_property_key(host, ku_t, (KEY), depth, thrown, ku_s, &(DST)); \\\n\
    if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_property_refusal(kiln_val_to_boundary(ku_t)); \\\n\
    KU_STATUS((PC), (OP), UNWIND); \\\n\
} while (0)\n");
    }
    if has_write_property {
        output.push_str("static __attribute__((unused, noinline)) int ku_pic_poly_write(kiln_object *obj, kiln_c99_site *site, kiln_val val) {\n    const kiln_shape *shape = obj->shape;\n    uint32_t count = kiln_c99_site_count(site);\n    for (uint32_t i = 1; i < count; ++i) {\n        if (shape == kiln_c99_slot_key(&site->entries[i].shape) && site->entries[i].transition_to == NULL) {\n            uint32_t slot = site->entries[i].slot;\n            if (slot != UINT32_MAX && KILN_LIKELY(slot < obj->slot_capacity && !obj->frozen)) {\n                kiln_c99_prototype_touched(obj);\n                kiln_c99_heap_store_val(obj, &obj->slots[slot], val);\n                kiln_c99_site_answered(site, i);\n                return 1;\n            }\n        }\n    }\n    return 0;\n}\n");
        output.push_str("#define KU_WRITE_PROPERTY(OBJECT, KEY, VAL, STRICT, SITE, PC, OP, UNWIND) do { \\\n\
    kiln_c99_site *ku_s = (SITE); \\\n\
    kiln_val ku_t = (OBJECT); \\\n\
    kiln_val ku_v = (VAL); \\\n\
    if (KILN_LIKELY(ku_s != NULL && kiln_c99_site_count(ku_s) != 0 && KILN_VAL_IS_OBJECT(ku_t))) { \\\n\
        kiln_object *ku_obj = (kiln_object *)KILN_VAL_PTR(ku_t); \\\n\
        if (KILN_LIKELY(ku_obj != NULL && ku_obj->shape != NULL && ku_obj->shape == kiln_c99_slot_key(&ku_s->entries[0].shape) && ku_s->entries[0].transition_to == NULL && !ku_obj->frozen)) { \\\n\
            uint32_t ku_s0 = ku_s->entries[0].slot; \\\n\
            if (KILN_LIKELY((ku_obj->shape->double_slots_mask & (1u << ku_s0)) == 0 || KILN_VAL_IS_NUMBER(ku_v))) { \\\n\
                if (KILN_LIKELY(ku_s0 < ku_obj->slot_capacity)) { \\\n\
                    kiln_c99_prototype_touched(ku_obj); \\\n\
                    kiln_c99_heap_store_val(ku_obj, &ku_obj->slots[ku_s0], ku_v); \\\n\
                    break; \\\n\
                } \\\n\
            } \\\n\
        } else if (ku_obj != NULL && kiln_c99_site_count(ku_s) > 1) { \\\n\
            if (ku_pic_poly_write(ku_obj, ku_s, ku_v)) break; \\\n\
        } \\\n\
    } \\\n\
    int status = kiln_c99_val_write_property_key(host, ku_t, (KEY), ku_v, (STRICT), depth, thrown, ku_s); \\\n\
    KU_STATUS((PC), (OP), UNWIND); \\\n\
} while (0)\n");
    }
    if !program.functions.is_empty() {
        output.push_str("#define KU_PARAMS kiln_call_context *ctx, kiln_val * restrict result, kiln_val * restrict upvalues, const kiln_val * restrict receiver_ptr, const kiln_val * restrict args, size_t argc\n");
        output.push_str("#define KU_EPILOGUE() kiln_unwind:; return KILN_C99_THROWN; kiln_refusal:; *result = kiln_val_from_boundary(kiln_c99_control_diagnostic()); return KILN_C99_CONTROL_ERROR;\n");
    }
    if has_call {
        output.push_str("static __attribute__((unused)) inline int ku_call_fast(kiln_host *host, kiln_val callee, const kiln_val *args, size_t argc, int depth, kiln_value *thrown, const char *site, size_t pc, const char *op, kiln_val *dst) {\n\
    if (KILN_LIKELY(KILN_VAL_IS_CLOSURE(callee))) {\n\
        kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(callee);\n\
        if (KILN_LIKELY(plain != NULL && !plain->publishes_frame && !plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH)) {\n\
            kiln_val recv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED;\n\
            kiln_c99_normalize_closure_receiver(&recv, plain);\n\
            kiln_host call_host; kiln_call_context ctx_call = { kiln_c99_closure_host(host, plain, &call_host), thrown, NULL, depth + 1 };\n\
            kiln_val res = KILN_VAL_UNDEFINED;\n\
            kiln_js_frame call_frame;\n\
            kiln_c99_js_frame_enter(&call_frame, plain);\n\
            int status = plain->body(&ctx_call, &res, plain->upvalues, &recv, args, argc);\n\
            kiln_c99_js_frame_leave(&call_frame);\n\
            if (KILN_LIKELY(status == KILN_C99_OK)) {\n\
                *dst = res;\n\
                return KILN_C99_OK;\n\
            }\n\
            if (status == KILN_C99_THROWN || status == KILN_C99_CONTROL_ERROR) {\n\
                if (kiln_c99_diagnostic_function == NULL && kiln_c99_diagnostic_function_key == NULL && plain != NULL) {\n\
                    kiln_c99_diagnostic_function = NULL;\n\
                    kiln_c99_diagnostic_function_len = 0;\n\
                    kiln_c99_diagnostic_function_key = plain->name;\n\
                }\n\
                *dst = res;\n\
                return status;\n\
            }\n\
            return kiln_c99_val_call(host, callee, args, argc, depth, thrown, site, pc, op, dst);\n\
        }\n\
    }\n\
    return kiln_c99_val_call(host, callee, args, argc, depth, thrown, site, pc, op, dst);\n\
}\n");
        output.push_str("#define KU_CALL(DST, CALLEE, ARGS, ARGC, PC, OP, UNWIND) do { \\\n\
    int status = ku_call_fast(host, (CALLEE), (ARGS), (size_t)(ARGC), depth, thrown, ku_site, (size_t)(PC), (OP), &(DST)); \\\n\
    if (KILN_UNLIKELY(status != KILN_C99_OK)) { \\\n\
        if (status == KILN_C99_THROWN) { \\\n\
            if (kiln_c99_diagnostic_detail == NULL) { \\\n\
                kiln_c99_diagnostic_receiver_tag = KILN_VALUE_FUNCTION; \\\n\
                kiln_c99_diagnostic_detail = \"Call invocation threw\"; \\\n\
            } \\\n\
            kiln_c99_note_refusal(ku_site, (size_t)(PC), (OP), (size_t)kiln_c99_diagnostic_active_depth); \\\n\
            UNWIND; \\\n\
        } \\\n\
        if (status == KILN_C99_CONTROL_ERROR) { \\\n\
            kiln_c99_note_refusal(ku_site, (size_t)(PC), (OP), (size_t)kiln_c99_diagnostic_active_depth); \\\n\
            goto kiln_refusal; \\\n\
        } \\\n\
        return status; \\\n\
    } \\\n\
} while (0)\n");
    }
    if has_call_method {
        output.push_str("static __attribute__((unused)) inline int ku_call_method_fast(kiln_host *host, kiln_val ku_recv, const kiln_val *args, size_t argc, const kiln_string *key, kiln_c99_site *ku_s, const char *ku_site, size_t pc, const char *op, int depth, kiln_value *thrown, kiln_val *dst) {\n\
    if (KILN_LIKELY(ku_s != NULL)) {\n\
        if (KILN_UNLIKELY(kiln_c99_cached_method_matches(ku_s, ku_recv, key) && depth < KILN_C99_MAX_CALL_DEPTH)) {\n\
            return kiln_c99_val_invoke_cached_method(host, ku_s->cached_method, ku_recv, args, argc, depth, thrown, dst);\n\
        }\n\
        if (KILN_LIKELY(KILN_VAL_IS_OBJECT(ku_recv) && kiln_c99_site_count(ku_s) > 0)) {\n\
            const kiln_object *obj = (const kiln_object *)KILN_VAL_PTR(ku_recv);\n\
            kiln_val cached_fn = kiln_c99_site_cached_callee(ku_s, obj);\n\
            if (KILN_LIKELY(cached_fn != 0 && KILN_VAL_IS_CLOSURE(cached_fn))) {\n\
                kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(cached_fn);\n\
                if (KILN_LIKELY(plain != NULL && !plain->publishes_frame && !plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH)) {\n\
                    kiln_val vreceiver = plain->arrow ? plain->captured_this : ku_recv;\n\
                    kiln_c99_normalize_closure_receiver(&vreceiver, plain);\n\
                    kiln_host call_host; kiln_call_context ctx_call = { kiln_c99_closure_host(host, plain, &call_host), thrown, NULL, depth + 1 };\n\
                    kiln_val res = KILN_VAL_UNDEFINED;\n\
                    kiln_js_frame call_frame;\n\
                    kiln_c99_js_frame_enter(&call_frame, plain);\n\
                    int status = plain->body(&ctx_call, &res, plain->upvalues, &vreceiver, args, argc);\n\
                    kiln_c99_js_frame_leave(&call_frame);\n\
                    if (KILN_LIKELY(status == KILN_C99_OK)) {\n\
                        *dst = res;\n\
                        return KILN_C99_OK;\n\
                    }\n\
                    if (status == KILN_C99_THROWN) {\n\
                        if (kiln_c99_diagnostic_detail == NULL) {\n\
                            kiln_c99_diagnostic_receiver_tag = kiln_val_to_boundary(ku_recv).tag;\n\
                            kiln_c99_diagnostic_detail = \"CallMethod invocation threw\";\n\
                        }\n\
                        *dst = res;\n\
                        return KILN_C99_THROWN;\n\
                    }\n\
                    if (status == KILN_C99_CONTROL_ERROR) {\n\
                        if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = kiln_val_to_boundary(ku_recv).tag;\n\
                        if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = \"CallMethod invocation refused\";\n\
                        *dst = res;\n\
                        return KILN_C99_CONTROL_ERROR;\n\
                    }\n\
                    return kiln_c99_val_call_method_key(host, ku_recv, args, argc, key, depth, thrown, ku_s, ku_site, pc, op, dst);\n\
                }\n\
            } else if (cached_fn != 0 && KILN_VAL_IS_METHOD(cached_fn) && depth < KILN_C99_MAX_CALL_DEPTH) {\n\
                return kiln_c99_val_invoke_cached_method(host, KILN_VAL_METHOD_ID(cached_fn), ku_recv, args, argc, depth, thrown, dst);\n\
            }\n\
        }\n\
    }\n\
    return kiln_c99_val_call_method_key(host, ku_recv, args, argc, key, depth, thrown, ku_s, ku_site, pc, op, dst);\n\
}\n");
        output.push_str("#define KU_CALL_METHOD(DST, OBJ, ARGS, ARGC, KEY, SITE, PC, OP, UNWIND) do { \\\n\
    int status = ku_call_method_fast(host, (OBJ), (ARGS), (size_t)(ARGC), (KEY), (SITE), ku_site, (size_t)(PC), (OP), depth, thrown, &(DST)); \\\n\
    KU_STATUS((PC), (OP), UNWIND); \\\n\
} while (0)\n");
    }
    if has_closure {
        output.push_str(&format!("#define KU_CLOSURE(DST, PROTO, RECV, CAP, CAP_CNT) do {{ int status = {closure_fn}(&(DST), (PROTO), (RECV), (CAP), (CAP_CNT)); if (KILN_UNLIKELY(status != KILN_C99_OK)) return status; }} while (0)\n"));
        if module_cells::region_site_count() > 0 {
            output.push_str(&format!("#define KU_REGION_CLOSURE(DST, PROTO, RECV, SITE) do {{ int status = {closure_fn}_region(&(DST), (PROTO), (RECV), (SITE)); if (KILN_UNLIKELY(status != KILN_C99_OK)) return status; }} while (0)\n"));
        }
    }
    if has_new_object {
        output.push_str("static __attribute__((unused)) inline uint32_t ku_compute_literal_mask(const kiln_val *pairs, size_t count) {\n\
    uint32_t mask = 0;\n\
    for (size_t i = 0; i < count; ++i) {\n\
        if (KILN_VAL_IS_NUMBER(pairs[i * 2 + 1])) mask |= (1u << i);\n\
    }\n\
    return mask;\n\
}\n\
static __attribute__((unused)) inline int ku_new_object_literal_fast(kiln_val *out, const kiln_shape **lit_shape, const kiln_val *pairs, size_t count) {\n\
    const kiln_shape *shape = __atomic_load_n(lit_shape, __ATOMIC_ACQUIRE);\n\
    if (KILN_LIKELY(shape != NULL && count <= KILN_C99_INLINE_SLOTS)) {\n\
        uint32_t mask = ku_compute_literal_mask(pairs, count);\n\
        if (KILN_LIKELY(shape->double_slots_mask == mask)) {\n\
            kiln_value built;\n\
            int status = kiln_c99_new_object(&built);\n\
            if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;\n\
            kiln_object *obj = kiln_c99_object_of(built);\n\
            obj->shape = (kiln_shape *)shape;\n\
            for (size_t i = 0; i < count; ++i) {\n\
                kiln_c99_heap_store_val(obj, &obj->inline_slots[i], pairs[i * 2 + 1]);\n\
            }\n\
            *out = kiln_val_from_boundary(built);\n\
            return KILN_C99_OK;\n\
        }\n\
    }\n\
    int status = kiln_c99_val_new_object_literal(out, pairs, count);\n\
    if (KILN_LIKELY(status == KILN_C99_OK && count <= KILN_C99_INLINE_SLOTS && KILN_VAL_IS_OBJECT(*out))) {\n\
        kiln_object *obj = (kiln_object *)KILN_VAL_PTR(*out);\n\
        if (obj->shape != NULL && KILN_C99_SHAPE_IS_PORTABLE(obj->shape) && obj->shape->property_count == count && obj->slots == obj->inline_slots && obj->properties == NULL && obj->elements == NULL && obj->prototype.tag == KILN_VALUE_UNDEFINED) {\n\
            __atomic_store_n(lit_shape, (const kiln_shape *)obj->shape, __ATOMIC_RELEASE);\n\
        }\n\
    }\n\
    return status;\n\
}\n");
    }
}

fn render(
    program: &Program,
    program_hash: u64,
    symbol: &str,
    policy: &EmissionPolicy,
) -> Result<(String, bool), EmitError> {
    parallel::with_original_source(program, || {
        render_positioned(program, program_hash, symbol, policy)
    })
}

fn render_positioned(
    program: &Program,
    program_hash: u64,
    symbol: &str,
    policy: &EmissionPolicy,
) -> Result<(String, bool), EmitError> {
    let plan = validate_program(program).expect("render called after validation");
    let mut compacted;
    let program = if policy.compact || policy.omit_source {
        compacted = program.clone();
        compact_closure_source(&mut compacted, None, policy.omit_source);
        &compacted
    } else {
        program
    };
    // A bare `Program` arrives with no goal and no strictness table: it is a scalar unit, and
    // every one this backend is handed came from a Module goal. Strict throughout keeps its
    // emitted bytes exactly what they were.
    let strict_functions = vec![true; program.functions.len()];
    let callables = produces_references(program);
    let header_len = if policy.decoupled_prelude {
        64
    } else {
        HEADER.len()
    };
    let mut output = String::with_capacity(header_len + program.code.len() * 64);
    render_emission_policy(&mut output, policy);
    if policy.decoupled_prelude {
        writeln!(output, "#include \"{RUNTIME_PRELUDE_HEADER_NAME}\"").unwrap();
    } else {
        output.push_str(HEADER);
        output = output.replace(
            "/* KILN_NATIVE_CONSTRUCTOR_TABLE */",
            &render_native_constructor_table(),
        );
    }
    writeln!(
        output,
        "/* Program hash: {program_hash:016x}; bytecode version: {BYTECODE_VERSION}. */"
    )
    .unwrap();
    render_constant_declarations(&mut output, program, symbol, policy.compact, false);
    if policy.should_compile_regex() {
        render_regex_matchers(&mut output, program, symbol, false);
    }
    let keys = property_key_constants(program);
    render_key_table(&mut output, program, symbol, &keys, false);
    render_unit_state(
        &mut output,
        symbol,
        unit_global_count(program),
        program.template_sites.len(),
        module_cells::compact_cell_count(),
        false,
        false,
        false,
    );
    render_template_sites(&mut output, program, symbol, false);
    render_unit_roots(&mut output, symbol, !keys.is_empty());
    if policy.compact {
        output.push_str("/* KILN_C99_SITES_PLACEHOLDER */\n");
    }
    let emission_diagnostic_site = if policy.compact {
        writeln!(
            output,
            "static const char ku_importer[] = {};",
            c_string(symbol)
        )
        .unwrap();
        writeln!(
            output,
            "static const char ku_site[] = {};",
            c_string(symbol)
        )
        .unwrap();
        render_compact_macros(&mut output, program, "ku_make_closure");
        COMPACT_DIAGNOSTIC_SITE
    } else {
        symbol
    };
    let extra_entry_slots = if uses_module_scope(program) {
        Some((0..program.local_count).collect::<HashSet<u32>>())
    } else {
        None
    };
    let (graphs, body, entry_graph, mut direct) = plan_unit(
        program,
        emission_diagnostic_site,
        policy.compact,
        policy.should_compile_regex(),
        policy.generic_only,
        policy.parallel_in_generic,
        policy.speculate,
        extra_entry_slots.as_ref(),
        None,
    )?;
    if program.has_dynamic_import() {
        output.push_str(
            "__attribute__((weak)) int kiln_graph_dynamic_import(kiln_host *host, const char *importer, kiln_value specifier, kiln_value options, kiln_value *promise, int depth, kiln_value *thrown) {\n    (void)importer; (void)specifier; (void)options;\n    kiln_value err = kiln_c99_undefined();\n    (void)kiln_c99_throw_error(&err, KILN_C99_BUILTIN_TYPE_ERROR, \"no module loader is installed\");\n    return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);\n}\n",
        );
    }
    let rendered_funcs = render_functions(
        &mut output,
        program,
        &plan,
        symbol,
        emission_diagnostic_site,
        &strict_functions,
        &graphs,
        &mut direct,
        policy,
        None,
        false,
        None,
        None,
    );
    let generic_mixed_optimization = direct
        .functions
        .iter()
        .any(|function| function.generic_function_optimized);
    let func_sites = rendered_funcs.sites;
    let entry_sites = render_entry(
        &mut output,
        program,
        &plan,
        symbol,
        symbol,
        emission_diagnostic_site,
        &strict_functions,
        !keys.is_empty(),
        None,
        &body,
        &entry_graph,
        &direct.entry,
        policy,
        false,
        func_sites,
    )?;
    let total_sites = func_sites + entry_sites;
    if policy.compact {
        let decl = if total_sites > 0 {
            format!("static kiln_c99_site {symbol}_sites[{total_sites}] = {{0}};\n")
        } else {
            String::new()
        };
        output = output.replace("/* KILN_C99_SITES_PLACEHOLDER */\n", &decl);
    }
    if callables {
        render_scalar_realm_wrapper(&mut output, symbol);
    }
    if policy.compact {
        prune_unused_compact_globals(&mut output, symbol, symbol, symbol, program);
    }
    Ok((output, generic_mixed_optimization))
}

pub use kiln_graph::{
    closed_world_function_liveness, closed_world_module_live_slots,
    closed_world_module_live_slots_with_declarations, closed_world_namespace_members,
    retained_module_instructions, retained_module_size,
};

fn scan_referenced_functions(texts: &[String], symbol: &str, total_funcs: usize) -> Vec<bool> {
    let mut referenced = vec![false; total_funcs];
    let prefix = format!("{symbol}_fn_");
    let pmap_prefix = format!("{symbol}_pmap_");
    let prefix_bytes = prefix.as_bytes();
    let pmap_bytes = pmap_prefix.as_bytes();
    if prefix_bytes.is_empty() {
        return referenced;
    }
    let first_byte = prefix_bytes[0];

    for text in texts {
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let Some(pos) = memchr::memchr(first_byte, &bytes[i..]) else {
                break;
            };
            i += pos;
            if bytes[i..].starts_with(prefix_bytes) {
                i += prefix_bytes.len();
                let mut num = 0usize;
                let mut has_digit = false;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    has_digit = true;
                    num = num
                        .saturating_mul(10)
                        .saturating_add((bytes[i] - b'0') as usize);
                    i += 1;
                }
                if has_digit && num < total_funcs {
                    referenced[num] = true;
                }
            } else if bytes[i..].starts_with(pmap_bytes) {
                i += pmap_bytes.len();
                let mut cursor = i;
                if cursor < bytes.len() && !bytes[cursor].is_ascii_digit() {
                    while cursor < bytes.len() && bytes[cursor] != b'_' {
                        cursor += 1;
                    }
                    if cursor < bytes.len() && bytes[cursor] == b'_' {
                        cursor += 1;
                    }
                }
                let mut num = 0usize;
                let mut has_digit = false;
                while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                    has_digit = true;
                    num = num
                        .saturating_mul(10)
                        .saturating_add((bytes[cursor] - b'0') as usize);
                    cursor += 1;
                }
                if has_digit && num < total_funcs {
                    referenced[num] = true;
                }
                i = cursor;
            } else {
                i += 1;
            }
        }
    }
    referenced
}

fn render_module(
    module: &ModuleProgram,
    program_hash: u64,
    symbol: &str,
    policy: &EmissionPolicy,
    commonjs: bool,
    diagnostic_site: &str,
) -> Result<RenderedModule, EmitError> {
    render_module_with_unit(
        module,
        program_hash,
        symbol,
        None,
        false,
        false,
        policy,
        commonjs,
        diagnostic_site,
        false,
        None,
    )
}

fn render_module_with_unit(
    module: &ModuleProgram,
    program_hash: u64,
    symbol: &str,
    shared_unit_symbol: Option<&str>,
    external_unit: bool,
    export_cells: bool,
    policy: &EmissionPolicy,
    commonjs: bool,
    diagnostic_site: &str,
    force_module_scope: bool,
    generic_loop_profile: Option<Arc<direct::LoopCallProfile>>,
) -> Result<RenderedModule, EmitError> {
    // `parallelMap` site records name positions in the module as written; emission packs the
    // source below. See `parallel::SiteRecord`.
    parallel::with_original_source(&module.program, || {
        render_module_positioned(
            module,
            program_hash,
            symbol,
            policy,
            commonjs,
            diagnostic_site,
            shared_unit_symbol,
            external_unit,
            export_cells,
            force_module_scope,
            generic_loop_profile,
        )
    })
}

fn render_module_positioned(
    module: &ModuleProgram,
    program_hash: u64,
    symbol: &str,
    policy: &EmissionPolicy,
    commonjs: bool,
    diagnostic_site: &str,
    shared_unit_symbol: Option<&str>,
    external_unit: bool,
    export_cells: bool,
    force_module_scope: bool,
    generic_loop_profile: Option<Arc<direct::LoopCallProfile>>,
) -> Result<RenderedModule, EmitError> {
    let program = &module.program;
    let should_chunk =
        policy.compact && (program.functions.len() > 500 || policy.unit_chunk_bytes.is_some());
    // Compact graph modules are each compiled as their own translation unit. Private identifiers
    // therefore do not need the link-visible content hash; retain it only on exported adapters.
    // Chunked modules must retain the module symbol prefix so their non-static chunk symbols
    // do not collide with other chunked modules in the same binary.
    let default_unit_symbol = if policy.compact && !should_chunk {
        "ku"
    } else {
        symbol
    };
    let unit_symbol = shared_unit_symbol.unwrap_or(default_unit_symbol);
    let plan = validate_module_program(program)?;
    // Split ESM modules render evaluation and instantiation as separate translation units that
    // share `unit_symbol`. Even an entry with no closures needs a private body symbol in each
    // unit; otherwise both units define the public `unit_symbol` entrypoint. The realm wrapper
    // below owns that public symbol and dispatches to this phase's body.
    let split_module_phase = external_unit || export_cells;
    let callables = produces_references(program) || policy.compact || split_module_phase;
    // What the compiler said, padded with the strict answer. A `ModuleProgram` assembled by hand
    // -- a builtin's export shape, a fixture -- carries no table, and reading its absence as
    // strict is what keeps every unit that is really ESM emitting the bytes it emitted before.
    let strict_functions: Vec<bool> = (0..program.functions.len())
        .map(|index| {
            module
                .strictness
                .functions
                .get(index)
                .copied()
                .unwrap_or(true)
        })
        .collect();
    let header_len = if policy.decoupled_prelude {
        64
    } else {
        HEADER.len()
    };
    let mut output = String::with_capacity(header_len + program.code.len() * 72);
    render_emission_policy(&mut output, policy);
    if policy.decoupled_prelude {
        writeln!(output, "#include \"{RUNTIME_PRELUDE_HEADER_NAME}\"").unwrap();
    } else {
        output.push_str(HEADER);
        output = output.replace(
            "/* KILN_NATIVE_CONSTRUCTOR_TABLE */",
            &render_native_constructor_table(),
        );
    }
    writeln!(
        output,
        "/* Module hash: {program_hash:016x}; bytecode version: {BYTECODE_VERSION}; imports: {}; exports: {}. */",
        module.imports.len(),
        if commonjs { 1 } else { module.exports.len() }
    )
    .unwrap();
    let closure_fn = if should_chunk {
        format!("{unit_symbol}_make_closure")
    } else {
        "ku_make_closure".to_string()
    };
    let emission_diagnostic_site = if policy.compact {
        writeln!(
            output,
            "static const char ku_importer[] = {};",
            c_string(symbol)
        )
        .unwrap();
        writeln!(
            output,
            "static const char ku_site[] = {};",
            c_string(diagnostic_site)
        )
        .unwrap();
        render_compact_macros(&mut output, program, &closure_fn);
        COMPACT_DIAGNOSTIC_SITE
    } else {
        diagnostic_site
    };
    render_constant_declarations(
        &mut output,
        program,
        unit_symbol,
        policy.compact,
        should_chunk,
    );
    if policy.should_compile_regex() {
        render_regex_matchers(&mut output, program, unit_symbol, should_chunk);
    }
    let keys = property_key_constants(program);
    render_key_table(&mut output, program, unit_symbol, &keys, should_chunk);
    if policy.shared_region {
        render_name_registration(&mut output, program, unit_symbol, &keys);
    }
    render_unit_state(
        &mut output,
        unit_symbol,
        unit_global_count(program).max(if force_module_scope {
            usize::try_from(program.local_count).unwrap().max(1)
        } else {
            0
        }),
        program.template_sites.len(),
        module_cells::compact_cell_count(),
        should_chunk,
        shared_unit_symbol.is_some_and(|shared| shared != symbol),
        external_unit && shared_unit_symbol.is_some_and(|shared| shared == symbol),
    );
    render_template_sites(&mut output, program, unit_symbol, should_chunk);
    render_unit_roots(&mut output, unit_symbol, !keys.is_empty());
    let region_sites = module_cells::region_site_count();
    if region_sites > 0 {
        // Each region site's closure (realms issue 08): written once, by the realm that builds
        // the region, before it publishes the region sealed; read by every realm after
        // (`kiln_c99_region_closure`).
        let linkage = if should_chunk { "" } else { "static " };
        writeln!(
            output,
            "{linkage}kiln_closure *{unit_symbol}_region_sites[{region_sites}];"
        )
        .unwrap();
    }
    if policy.compact {
        output.push_str("/* KILN_C99_SITES_PLACEHOLDER */\n");
    }
    // A module-wide syntax fact is deliberately conservative for linking, but it is too broad
    // for native emission: a direct eval after an unconditional return cannot execute and must
    // not disable closed-world function liveness for the whole module. Use the same CFG that
    // drives emission to distinguish unreachable eval bytecode from a live eval (which remains
    // unsupported below).
    let mut reachable_program = program.clone();
    let live_direct_eval = prune_unreachable_direct_eval(&mut reachable_program);
    let liveness = policy.live_module_slots.as_ref().map(|demanded| {
        closed_world_function_liveness(
            &reachable_program,
            demanded,
            live_direct_eval,
            &module.link_facts.removable_declarations,
        )
    });
    let mut pruned = reachable_program;
    if let Some((_, dead_pcs, _)) = &liveness {
        for (pc, instruction) in pruned.code.iter_mut().enumerate() {
            if dead_pcs[0].contains(&(pc as u32)) {
                *instruction = Instruction::Nop;
            }
        }
        for (index, function) in pruned.functions.iter_mut().enumerate() {
            for (pc, instruction) in function.code.iter_mut().enumerate() {
                if dead_pcs[index + 1].contains(&(pc as u32)) {
                    *instruction = Instruction::Nop;
                }
            }
        }
    }
    // Direct eval is admitted to the shared dynamic expression seam. Unsupported runtime source
    // refuses at the actual instruction, while supported factories close over caller cells.
    if policy.compact || policy.omit_source {
        compact_closure_source(
            &mut pruned,
            liveness.as_ref().map(|(reachable, _, _)| reachable),
            policy.omit_source,
        );
    }
    let emitted_program = if policy.compact || policy.omit_source || liveness.is_some() {
        &pruned
    } else {
        program
    };
    let extra_entry_slots: HashSet<u32> =
        if force_module_scope || uses_module_scope(emitted_program) {
            (0..emitted_program.local_count).collect()
        } else {
            let mut slots = HashSet::new();
            slots.extend(module.imports.iter().map(|binding| binding.local_slot));
            slots.extend(module.exports.iter().map(|binding| binding.local_slot));
            if commonjs {
                slots.insert(2);
            }
            slots
        };
    let (graphs, body, entry_graph, mut direct) = plan_unit(
        emitted_program,
        diagnostic_site,
        policy.compact,
        policy.should_compile_regex(),
        policy.generic_only,
        policy.parallel_in_generic,
        policy.speculate,
        Some(&extra_entry_slots),
        generic_loop_profile,
    )?;
    if should_chunk {
        direct.set_should_chunk(true);
    }
    let reachable = liveness.as_ref().map(|(reachable, _, _)| reachable);
    if let Some(reachable) = reachable {
        let available = (0..emitted_program.functions.len())
            .map(|index| reachable.contains(&index))
            .collect::<Vec<_>>();
        direct.set_available(emitted_program, &available);
    }
    if emitted_program.has_dynamic_import() {
        output.push_str(
            "__attribute__((weak)) int kiln_graph_dynamic_import(kiln_host *host, const char *importer, kiln_value specifier, kiln_value options, kiln_value *promise, int depth, kiln_value *thrown) {\n    (void)importer; (void)specifier; (void)options;\n    kiln_value err = kiln_c99_undefined();\n    (void)kiln_c99_throw_error(&err, KILN_C99_BUILTIN_TYPE_ERROR, \"no module loader is installed\");\n    return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);\n}\n",
        );
    }
    let mut chunked_bodies = if should_chunk {
        Some(Vec::with_capacity(emitted_program.functions.len()))
    } else {
        None
    };
    // Issue 14: a generic-only unit is compiled for build speed, so the parallel clones and
    // chunk workers of its qualifying sites -- the only code in it that runs per element on a
    // pool thread -- go to a unit of their own that the orchestrator compiles optimised. It
    // holds nothing else: a worker reaches no heap, realm or module state (`lib.rs` scans
    // every clone for that before admitting it), so the prelude's declarations are all it needs.
    let split_workers =
        policy.generic_only && policy.parallel_workers_unit && policy.decoupled_prelude;
    let mut workers_unit = String::new();
    let rendered_funcs = render_functions(
        &mut output,
        emitted_program,
        &plan,
        unit_symbol,
        emission_diagnostic_site,
        &strict_functions,
        &graphs,
        &mut direct,
        policy,
        reachable,
        should_chunk,
        chunked_bodies.as_mut(),
        split_workers.then_some((symbol, &mut workers_unit)),
    );
    let generic_optimized_functions = direct
        .functions
        .iter()
        .enumerate()
        .map(|(index, plan)| {
            plan.generic_function_optimized && reachable.is_none_or(|set| set.contains(&index))
        })
        .collect::<Vec<_>>();
    let generic_mixed_optimization = generic_optimized_functions.iter().any(|&selected| selected);
    let parallel_workers = (!workers_unit.is_empty()).then(|| {
        let mut unit = String::with_capacity(workers_unit.len() + 256);
        render_emission_policy(&mut unit, policy);
        writeln!(unit, "#include \"{RUNTIME_PRELUDE_HEADER_NAME}\"").unwrap();
        writeln!(
            unit,
            "/* Parallel workers of {symbol} (module hash {program_hash:016x}), compiled apart from its generic bodies. */"
        )
        .unwrap();
        unit.push_str(&workers_unit);
        unit
    });
    let func_sites = rendered_funcs.sites;
    let entry_sites = render_entry(
        &mut output,
        emitted_program,
        &plan,
        symbol,
        unit_symbol,
        emission_diagnostic_site,
        &strict_functions,
        !keys.is_empty(),
        Some((module, commonjs)),
        &body,
        &entry_graph,
        &direct.entry,
        policy,
        external_unit || export_cells,
        func_sites,
    )?;
    let total_sites = func_sites + entry_sites;
    if policy.compact {
        let decl = if total_sites > 0 {
            let linkage = if should_chunk { "" } else { "static " };
            format!("{linkage}kiln_c99_site {unit_symbol}_sites[{total_sites}] = {{0}};\n")
        } else {
            String::new()
        };
        output = output.replace("/* KILN_C99_SITES_PLACEHOLDER */\n", &decl);
    }
    if commonjs {
        /* The launcher cannot manufacture native objects without coupling itself to this private
         * value layout. Let the translation unit that owns the layout construct the wrapper
         * values instead. The helper is linked only into native graphs; it is not part of the
         * public scalar embedding header or ABI. */
        writeln!(
            output,
            "int {symbol}_commonjs_wrapper(kiln_value *imports, size_t import_count, const char *filename, size_t filename_len, const char *dirname, size_t dirname_len, kiln_commonjs_require_fn require_call, kiln_commonjs_resolve_fn require_resolve, void *require_context, kiln_gc_visit_roots_fn require_visit_roots, void *require_root_context) {{\n    kiln_value initial;\n    kiln_value module;\n    int status;\n    if (imports == NULL || import_count != (size_t)5) return KILN_C99_INVALID_ARGUMENT;\n    status = kiln_c99_new_object(&initial);\n    if (status == KILN_C99_OK) imports[0] = initial;\n    if (status == KILN_C99_OK) status = kiln_c99_new_object(&module);\n    if (status == KILN_C99_OK) imports[2] = module;\n    if (status == KILN_C99_OK) status = kiln_c99_put(NULL, imports[2], \"exports\", 7, imports[0], 0, NULL);\n    if (status == KILN_C99_OK) status = kiln_c99_commonjs_require_value_with_resolver(&imports[1], require_call, require_resolve, require_context, require_visit_roots, require_root_context);\n    if (status != KILN_C99_OK) return status;\n    imports[3] = kiln_c99_copy_text(filename, filename_len);\n    imports[4] = kiln_c99_copy_text(dirname, dirname_len);\n    return KILN_C99_OK;\n}}"
        )
        .unwrap();
    }
    render_namespace_wrapper(&mut output, symbol);
    if callables {
        render_module_realm_wrapper(
            &mut output,
            symbol,
            unit_symbol,
            commonjs.then_some(diagnostic_site),
        );
    }
    if should_chunk {
        let function_texts = chunked_bodies.expect("chunked_bodies present when should_chunk");
        let total_funcs = function_texts.len();
        // Where each unit ends: by function count, or (issue 17) by bytes of C, greedily in
        // function order. Chunk 0 also holds the module's own text -- its body, key table,
        // constants and entry -- so it starts that much fuller.
        let chunk_ends: Vec<usize> = match policy.unit_chunk_bytes {
            Some(budget) => {
                let budget = budget.max(1);
                let mut ends = Vec::new();
                let mut filled = output.len();
                for (index, text) in function_texts.iter().enumerate() {
                    if filled > 0
                        && filled + text.len() > budget
                        && index > ends.last().copied().unwrap_or(0)
                    {
                        ends.push(index);
                        filled = 0;
                    }
                    filled += text.len();
                }
                ends.push(total_funcs);
                ends
            }
            None => {
                let chunk_size = if total_funcs > 10_000 { 1500 } else { 1000 };
                (1..)
                    .map(|k| (k * chunk_size).min(total_funcs))
                    .take(total_funcs.div_ceil(chunk_size).max(1))
                    .collect()
            }
        };
        let chunk_0_funcs = chunk_ends[0];

        let source = output.replace(
            "/* KILN_CHUNK_0_FUNCTIONS_PLACEHOLDER */\n",
            &function_texts.concat(),
        );
        let chunk_0 = output.replace(
            "/* KILN_CHUNK_0_FUNCTIONS_PLACEHOLDER */\n",
            &function_texts[0..chunk_0_funcs].concat(),
        );

        let mut chunk_header = String::with_capacity(32 * 1024);
        render_emission_policy(&mut chunk_header, policy);
        if policy.decoupled_prelude {
            writeln!(chunk_header, "#include \"{RUNTIME_PRELUDE_HEADER_NAME}\"").unwrap();
        } else {
            chunk_header.push_str(HEADER);
            chunk_header = chunk_header.replace(
                "/* KILN_NATIVE_CONSTRUCTOR_TABLE */",
                &render_native_constructor_table(),
            );
        }
        writeln!(
            chunk_header,
            "static const char ku_importer[] = {};",
            c_string(symbol)
        )
        .unwrap();
        writeln!(
            chunk_header,
            "static const char ku_site[] = {};",
            c_string(diagnostic_site)
        )
        .unwrap();
        // The weak fallback is defined in chunk 0, and the graph launcher can replace it.
        // Nested functions in other chunks still need its declaration at their call sites.
        if emitted_program.has_dynamic_import() {
            chunk_header.push_str(
                "int kiln_graph_dynamic_import(kiln_host *host, const char *importer, kiln_value specifier, kiln_value options, kiln_value *promise, int depth, kiln_value *thrown);\n",
            );
        }
        render_compact_macros(&mut chunk_header, emitted_program, &closure_fn);
        render_constant_declarations_extern(
            &mut chunk_header,
            emitted_program,
            unit_symbol,
            policy.compact,
            false,
        );
        render_key_table_extern(&mut chunk_header, unit_symbol, &keys);
        if policy.should_compile_regex() {
            render_regex_matchers_extern(&mut chunk_header, emitted_program, unit_symbol);
        }
        // The block's layout is the whole unit's: the program the descriptor was rendered from.
        render_unit_state(
            &mut chunk_header,
            unit_symbol,
            unit_global_count(program),
            program.template_sites.len(),
            module_cells::compact_cell_count(),
            true,
            true,
            false,
        );
        if total_sites > 0 {
            writeln!(chunk_header, "extern kiln_c99_site {unit_symbol}_sites[];").unwrap();
        }
        if region_sites > 0 {
            writeln!(
                chunk_header,
                "extern kiln_closure *{unit_symbol}_region_sites[];"
            )
            .unwrap();
        }
        let has_closure = has_closure_instruction(emitted_program);
        if has_closure && !emitted_program.functions.is_empty() {
            writeln!(
                chunk_header,
                "extern int {closure_fn}(kiln_val *dst, uint32_t proto, kiln_val receiver, const kiln_val *captured, uint32_t count);"
            )
            .unwrap();
            if region_sites > 0 {
                writeln!(
                    chunk_header,
                    "extern int {closure_fn}_region(kiln_val *dst, uint32_t proto, kiln_val receiver, kiln_closure **site);"
                )
                .unwrap();
            }
        }
        if !emitted_program.template_sites.is_empty() {
            writeln!(
                chunk_header,
                "extern int {unit_symbol}_materialize_template_site(kiln_host *host, size_t site);"
            )
            .unwrap();
        }

        let mut chunks = Vec::new();
        let mut chunk_generic_mixed_optimization = Vec::new();
        if chunk_ends.len() > 1 {
            chunks.push(chunk_0);
            chunk_generic_mixed_optimization.push(
                generic_optimized_functions[..chunk_0_funcs]
                    .iter()
                    .any(|&selected| selected),
            );
            let mut start = chunk_0_funcs;
            for &end in &chunk_ends[1..] {
                let referenced = scan_referenced_functions(
                    &function_texts[start..end],
                    unit_symbol,
                    total_funcs,
                );
                let decl_len: usize = (0..total_funcs)
                    .filter(|&i| (i >= start && i < end) || referenced[i])
                    .map(|i| rendered_funcs.function_declarations[i].len())
                    .sum();
                let bodies_len: usize = function_texts[start..end].iter().map(String::len).sum();
                let mut chunk_k = String::with_capacity(chunk_header.len() + decl_len + bodies_len);
                chunk_k.push_str(&chunk_header);
                for i in 0..total_funcs {
                    if (i >= start && i < end) || referenced[i] {
                        chunk_k.push_str(&rendered_funcs.function_declarations[i]);
                    }
                }
                for f in &function_texts[start..end] {
                    chunk_k.push_str(f);
                }
                chunks.push(chunk_k);
                chunk_generic_mixed_optimization.push(
                    generic_optimized_functions[start..end]
                        .iter()
                        .any(|&selected| selected),
                );
                start = end;
            }
        }
        Ok(RenderedModule {
            source,
            chunks,
            generic_mixed_optimization,
            chunk_generic_mixed_optimization,
            parallel_workers,
        })
    } else {
        if policy.compact {
            prune_unused_compact_globals(
                &mut output,
                symbol,
                unit_symbol,
                diagnostic_site,
                emitted_program,
            );
        }
        Ok(RenderedModule {
            source: output,
            chunks: Vec::new(),
            generic_mixed_optimization,
            chunk_generic_mixed_optimization: Vec::new(),
            parallel_workers,
        })
    }
}

fn render_runtime_module(
    module: &ModuleProgram,
    program_hash: u64,
    symbol: &str,
    body: &str,
    policy: &EmissionPolicy,
) -> String {
    let header_len = if policy.decoupled_prelude {
        64
    } else {
        HEADER.len()
    };
    let mut output = String::with_capacity(header_len + body.len() + 1024);
    render_emission_policy(&mut output, policy);
    if policy.decoupled_prelude {
        writeln!(output, "#include \"{RUNTIME_PRELUDE_HEADER_NAME}\"").unwrap();
    } else {
        output.push_str(HEADER);
        output = output.replace(
            "/* KILN_NATIVE_CONSTRUCTOR_TABLE */",
            &render_native_constructor_table(),
        );
    }
    writeln!(
        output,
        "/* Runtime module hash: {program_hash:016x}; bytecode version: {BYTECODE_VERSION}; imports: {}; exports: {}. */",
        module.imports.len(),
        module.exports.len()
    )
    .unwrap();
    writeln!(
        output,
        "static int {symbol}_body(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count) {{"
    )
    .unwrap();
    output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
    writeln!(
        output,
        "    if (import_count != (size_t){} || (import_count != 0 && imports == NULL)) return KILN_C99_INVALID_ARGUMENT;",
        module.imports.len()
    )
    .unwrap();
    writeln!(
        output,
        "    if (export_count != (size_t){} || (export_count != 0 && exports == NULL)) return KILN_C99_INVALID_ARGUMENT;",
        module.exports.len()
    )
    .unwrap();
    output.push_str("    (void)host;\n    (void)imports;\n");
    for line in body.lines() {
        output.push_str("    ");
        output.push_str(line);
        output.push('\n');
    }
    output.push_str("    *result = kiln_c99_undefined();\n    return KILN_C99_OK;\n}\n");
    writeln!(
        output,
        "void {symbol}_publish_exports(kiln_host *host, kiln_value *exports) {{\n    (void)host; (void)exports;\n}}"
    )
    .unwrap();
    render_namespace_wrapper(&mut output, symbol);
    render_module_realm_wrapper(&mut output, symbol, symbol, None);
    output
}

fn prune_unused_compact_globals(
    output: &mut String,
    symbol: &str,
    unit_symbol: &str,
    diagnostic_site: &str,
    program: &Program,
) {
    let importer_decl = format!("static const char ku_importer[] = {};\n", c_string(symbol));
    if output.matches("ku_importer").count() <= 1 {
        *output = output.replace(&importer_decl, "");
    }
    let site_decl = format!(
        "static const char ku_site[] = {};\n",
        c_string(diagnostic_site)
    );
    if output.matches("ku_site").count() <= 1 {
        *output = output.replace(&site_decl, "");
    }
    for (index, constant) in program.constants.iter().enumerate() {
        match constant {
            Constant::String(_) => {
                let const_def = format!(
                    "#define {unit_symbol}_constant_{index} (&{unit_symbol}_string_{index})\n"
                );
                if output
                    .matches(&format!("{unit_symbol}_constant_{index}"))
                    .count()
                    <= 1
                {
                    *output = output.replace(&const_def, "");
                }
            }
            Constant::BigInt(_) => {
                let const_def = format!(
                    "#define {unit_symbol}_constant_{index} (&{unit_symbol}_bigint_digits_{index})\n"
                );
                if output
                    .matches(&format!("{unit_symbol}_constant_{index}"))
                    .count()
                    <= 1
                {
                    *output = output.replace(&const_def, "");
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn instruction_diagnostic(instruction: &Instruction, constants: &[Constant]) -> String {
    kiln_graph::instruction_diagnostic(instruction, constants)
}

mod string;
pub(crate) use string::*;

fn unary_code(operator: UnaryOp) -> u16 {
    match operator {
        UnaryOp::Plus => 1,
        UnaryOp::Negate => 2,
        UnaryOp::Not => 3,
        // Stable: these are persisted cache keys, so a new operator takes the next free code
        // rather than renumbering the existing ones.
        UnaryOp::TypeOf => 4,
        UnaryOp::BitwiseNot => 5,
        UnaryOp::Void => 6,
        UnaryOp::ToNumeric => 7,
        UnaryOp::Increment => 8,
        UnaryOp::Decrement => 9,
        UnaryOp::ToString => 10,
    }
}

/// Whether this proto's own code reads the per-frame globals directly, ignoring anything it
/// calls or creates. The graph-aware form can discharge `LoadArguments` only when the exact
/// graph lowering fact proves every reachable load is replaced by `argc`.
///
/// `kiln_c99_invoke_closure_body` saves and restores `new_target`, `frame_args`, `frame_argc`,
/// `captured_arguments` and the arrow flag around every call. Two instructions read them --
/// `LoadArguments` and `LoadNewTarget` -- so a function containing neither cannot tell whether
/// the publication happened, *from its own code alone*. Skipping it measured 1.48x on a
/// recursive `fib`, which is call-bound and reads neither.
///
/// `RestArguments` is deliberately not in this list: the emitted C builds it from the body's own
/// `args` and `argc` parameters (see `tail.rs`'s `Instruction::RestArguments` arm), never the
/// frame globals, so `(...xs) => xs.length` does not need the frame merely for the rest
/// parameter.
///
/// A generator, async, or async-generator function is conservative unconditionally: its body is
/// entered and re-entered through paths that do not all pass through this predicate.
///
/// This is the *local* half of the answer. Whether a function needs the frame because of what it
/// creates is [`compute_publishes_frame`]'s fixpoint over the whole program, not this function --
/// an arrow that only reads `this` does not need the frame at all, so `arrow` alone is
/// deliberately not a term here (unlike the predicate this replaced).
fn proto_reads_frame_directly_with_argument_facts(
    proto: &FunctionProto,
    arguments_reads_elided: bool,
) -> bool {
    proto.generator
        || proto.asynchronous
        || proto.async_generator
        || proto.code.iter().any(|instruction| {
            matches!(instruction, Instruction::LoadNewTarget)
                || (matches!(instruction, Instruction::LoadArguments) && !arguments_reads_elided)
        })
}

/// Whether a call into each of `program.functions` must publish the per-frame context, computed
/// once for the whole program and threaded to every user -- `direct_callable`, the fast-clone and
/// direct-call eligibility checks in `emit_graph.rs`, and the `kiln_c99_closure_of(built)
/// ->publishes_frame` write at `MakeFunction` -- rather than recomputed per call site. The result
/// rides alongside `CallPlan::typed_entries` and friends, cloned into every function's plan.
///
/// Proto `i` publishes when its local frame-use fact says so, or when its code contains
/// `MakeFunction(j)` for an arrow `j` that itself publishes: `kiln_c99_make_closure` captures the
/// *enclosing* frame's visible `arguments`/`new.target` into an arrow's closure unconditionally
/// at construction time, so the frame must be live then whenever the new arrow will actually read
/// what it captured. A nested arrow that itself needs nothing from the frame captures values
/// nobody reads, which is inert rather than wrong; a nested *non-arrow* function has its own
/// `arguments` and never forces its parent, published or not.
///
/// Iterated to a fixpoint because an arrow can nest inside an arrow: each pass over the program
/// can only turn a `false` into a `true`, never back, so this settles in at most
/// `program.functions.len()` passes.
pub(crate) fn compute_publishes_frame(program: &Program) -> Vec<bool> {
    compute_publishes_frame_with_argument_facts(program, &[])
}

fn compute_publishes_frame_with_argument_facts(
    program: &Program,
    argument_facts: &[emit_graph::ArgumentsLengthFact],
) -> Vec<bool> {
    let mut publishes: Vec<bool> = program
        .functions
        .iter()
        .enumerate()
        .map(|(index, proto)| {
            let reads_elided = argument_facts
                .get(index)
                .is_some_and(|fact| fact.reads_elided);
            proto_reads_frame_directly_with_argument_facts(proto, reads_elided)
        })
        .collect();
    loop {
        let mut changed = false;
        for (i, proto) in program.functions.iter().enumerate() {
            if publishes[i] {
                continue;
            }
            let forces = proto.code.iter().any(|instruction| match instruction {
                Instruction::MakeFunction(j) => {
                    let j = *j as usize;
                    program.functions[j].arrow && publishes[j]
                }
                _ => false,
            });
            if forces {
                publishes[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    publishes
}

fn binary_code(operator: BinaryOp) -> u16 {
    match operator {
        BinaryOp::Add => 1,
        BinaryOp::Subtract => 2,
        BinaryOp::Multiply => 3,
        BinaryOp::Divide => 4,
        BinaryOp::Remainder => 5,
        BinaryOp::Exponentiate => 6,
        BinaryOp::StrictEqual => 7,
        BinaryOp::StrictNotEqual => 8,
        BinaryOp::LessThan => 9,
        BinaryOp::LessThanOrEqual => 10,
        BinaryOp::GreaterThan => 11,
        BinaryOp::GreaterThanOrEqual => 12,
        // Stable codes: a new operator takes the next free value.
        BinaryOp::In => 13,
        BinaryOp::InstanceOf => 14,
        BinaryOp::LooseEqual => 15,
        BinaryOp::LooseNotEqual => 16,
        BinaryOp::BitwiseAnd => 17,
        BinaryOp::BitwiseOr => 18,
        BinaryOp::BitwiseXor => 19,
        BinaryOp::ShiftLeft => 20,
        BinaryOp::ShiftRight => 21,
        BinaryOp::UnsignedShiftRight => 22,
    }
}

pub(crate) fn host_op_code(operation: HostOp) -> u16 {
    operation as u16
}

/// The call that reduces a host call's argument when it cannot cross the boundary, up to its
/// operand: a console write shows it as node's console does, `util.inspect.custom` included;
/// anything else as its plain inspection.
pub(crate) fn host_argument_inspection(operation: HostOp) -> &'static str {
    if matches!(operation, HostOp::WriteStdout | HostOp::WriteStderr) {
        "kiln_c99_console_inspect(host, "
    } else {
        "kiln_c99_inspect("
    }
}

mod identity;
use identity::*;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod parallel_tests;

/// The reserved key an instance stores its class's brand under, derived from a private name.
///
/// The first token is an opaque module-and-scope identity, so it becomes the brand key by
/// replacing only the final member token. One brand is shared by every private member declared
/// by that class. Derived here rather than passed, so the instruction keeps the shape the
/// interpreter gives it.
fn private_brand_key_of(name: &str) -> String {
    match name
        .strip_prefix("@@private:")
        .and_then(|rest| rest.split_once(':'))
    {
        Some((scope, _)) => format!("@@private:{scope}:%brand"),
        None => name.to_owned(),
    }
}
