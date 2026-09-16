//! Deterministic C99 emission for Kiln's scalar bytecode.
//!
//! This crate is deliberately a backend, not a C runtime.  The generated translation unit owns
//! the value representation and calls a host callback for every [`kiln_ir::Instruction::HostCall`]
//! using the numeric [`kiln_host_abi::HostOp`] value.  In particular, it never writes stdout (or
//! otherwise bakes CLI policy into an object file).

use std::{
    collections::{BTreeSet, HashSet},
    fmt::Write as _,
    sync::Arc,
};

use kiln_host_abi::{HOST_ABI_VERSION, HOST_OP_MODULE_CREATE_REQUIRE, HostOp};
use kiln_ir::{
    BYTECODE_VERSION, BinaryOp, Constant, FunctionProto, Instruction, ModuleProgram, Program,
    RemovableDeclaration, UnaryOp, Upvalue,
};
use kiln_native_number::{NUMBER_FORMAT_VERSION, NUMBER_PARSE_VERSION};

/// Version of the generated C ABI.  This is independent from the bytecode and HostOps versions.
pub const C_ABI_VERSION: u32 = 1;

/// Internal host callback code for realm-owned `createRequire` values. The emitted C macro and
/// generated launchers mirror the typed host-ABI authority rather than defining another number.
pub const C99_HOST_MODULE_CREATE_REQUIRE: u16 = HOST_OP_MODULE_CREATE_REQUIRE;

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
// value bridge and its provider-boundary codecs through generated C.
pub const C_GENERATOR_VERSION: u32 = 48;

/// Content identity of this emitter checkout, including dirty source changes. Input-keyed
/// caches must not trust a manually advanced generator epoch alone across active worktrees.
pub const C_EMITTER_BUILD_ID: &str = env!("KILN_C_EMITTER_BUILD_ID");

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
    /// Whether speculative versioning is enabled. Native orchestration resolves its process
    /// setting before emission; `None` preserves the direct emitter's default.
    pub speculate: Option<bool>,
    pub type_profile_record: bool,
    pub type_profile: Option<std::sync::Arc<graph::TypeProfile>>,
    /// Module slots whose exported values can be observed by this closed-world executable.
    /// `None` preserves the public module emitter's complete export surface.
    pub live_module_slots: Option<Arc<BTreeSet<u32>>>,
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
    ($group:ident, $name:ident, realm_cancel_request) => {
        concat!("int ", stringify!($name), "(uint64_t realm, uint64_t request);\n")
    };
    ($group:ident, $name:ident, realm_clear_timer) => {
        concat!("int ", stringify!($name), "(uint64_t id);\n")
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
    kiln_native_number_abi::provider_abi_registry!(provider_http_close_contract_count) == 1
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
    let source = render(program, program_hash, &symbol, &policy)?;
    Ok(CModule {
        symbol,
        source,
        program_hash,
        generic_only: policy.generic_only,
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
    let source = render_module(module, program_hash, &symbol, &policy, false, &symbol)?;
    Ok(CModule {
        symbol,
        source,
        program_hash,
        generic_only: policy.generic_only,
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
    let (symbol, program_hash, _) =
        module_identity_with_logical_path_and_policy(module, logical_path, &policy)?;
    let source = render_module(module, program_hash, &symbol, &policy, false, logical_path)?;
    Ok(CModule {
        symbol,
        source,
        program_hash,
        generic_only: policy.generic_only,
    })
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
    let source = render_module(module, program_hash, &symbol, &policy, true, logical_path)?;
    Ok(CModule {
        symbol,
        source,
        program_hash,
        generic_only: policy.generic_only,
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
pub mod parallel;
pub mod regex;
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
                    | Instruction::LoadCell(_)
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
        kiln_ir::Builtin::AggregateError => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_AGGREGATE_ERROR)")
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
        kiln_ir::Builtin::Url => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_URL)"),
        kiln_ir::Builtin::Buffer => BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_BUFFER)"),
        kiln_ir::Builtin::NotANumber => BuiltinLoad::Value("kiln_c99_number(0.0 / 0.0)"),
        kiln_ir::Builtin::Infinity => BuiltinLoad::Value("kiln_c99_number(1.0 / 0.0)"),
        kiln_ir::Builtin::AbortController => {
            BuiltinLoad::Value("kiln_c99_builtin(KILN_C99_BUILTIN_ABORT_CONTROLLER)")
        }
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
        #[allow(unreachable_patterns)]
        _ => unreachable!("verification rejects every other builtin before emission"),
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
) -> String {
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
    let generator = i32::from(target.generator || target.asynchronous);
    let asynchronous = i32::from(target.asynchronous);
    let arrow = i32::from(target.arrow);
    let async_generator = i32::from(target.async_generator);
    let is_method = i32::from(target.is_method);
    let strict = i32::from(
        strict_functions
            .get(proto as usize)
            .copied()
            .unwrap_or(true),
    );
    let arity = target.declared_length;
    let frame_locals = target.local_count.max(1);
    let frame_stack = frame_stacks
        .get(proto as usize)
        .copied()
        .unwrap_or(1)
        .max(1);
    let frame_handlers = target.code.len().max(1);
    let prologue_resume = if binds_at_call(target) {
        target.code.len()
    } else {
        0
    };
    let diagnostic_source = diagnostic_source(diagnostic_site);
    format!(
        "kiln_c99_make_closure(&built, {symbol}_fn_{proto}, {name}, {source}, {source_len}, {diagnostic_source}, UINT32_C({proto}), UINT32_C({arity}), {generator}, {asynchronous}, {async_generator}, {is_method}, {arrow}, {strict}, {defining_receiver}, kiln_val_from_boundary(kiln_c99_new_target), UINT32_C({frame_locals}), UINT32_C({frame_stack}), UINT32_C({frame_handlers}), UINT32_C({prologue_resume}), {captured}, UINT32_C({capture_count}))"
    )
}

/// Pack the source ranges observable through live closures and rewrite their spans into the pack.
///
/// A bundled module's original source commonly contains large top-level tables and functions that
/// closed-world emission removed. Keeping all of it as one escaped C string makes the C compiler
/// parse dead JavaScript. Ranges are unioned before copying because nested closures overlap their
/// parent's source; copying once per function would grow precisely the modules this is for.
fn compact_closure_source(program: &mut Program, reachable: Option<&HashSet<usize>>) {
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
            function.span.start = start as u32;
            function.span.end = end as u32;
        } else {
            // Preserve the old invalid-span answer (`NULL`, length zero) against the new source.
            function.span.start = 1;
            function.span.end = 0;
        }
    }
    program.source = packed.into();
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
) {
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
                );
            }
        };
    // What each typed entry needs is what its emitted body names, and what its typed callees
    // need. Emit the typed bodies to find out, until nothing more is needed.
    let mut needs = vec![direct::TypedNeeds::default(); program.functions.len()];
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
            let found = direct::TypedNeeds {
                runtime: names(body, "host") || names(body, "thrown"),
                upvalues: names(body, "upvalues"),
                receiver: names(body, "receiver"),
                row: names(body, "recv_row"),
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
    // A restartable body gets a fast clone: decided by emitting it, once, before any caller.
    // The parallel clones (ADR 0058, application 2): for every proto whose body passes
    // `parallel::parallel_safe` and whose typed entry returns a number, emit the typed body once
    // more under `parallel_clone()` -- named `_p`, `Math.<name>` as bare libm -- and admit it only
    // if its text names none of the host, `thrown`, the receiver, the upvalue table, a shape row,
    // or the collector: the same reading `typed_needs` does above, as the machine's own check
    // that a chunk worker calling this clone reaches nothing a realm thread owns.
    let mut parallel_clones: Vec<Option<String>> = vec![None; program.functions.len()];
    for (index, typed) in direct.typed.iter().enumerate() {
        let Some(typed) = typed else {
            continue;
        };
        if !(typed.typed_returns[index] && typed.parallel_safe[index]) {
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
                    returns_number: true,
                    restartable: false,
                },
                None,
                false,
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
        ]
        .iter()
        .any(|word| names(body, word));
        if clean {
            parallel_clones[index] = Some(scratch);
        }
    }
    let admitted: Vec<bool> = parallel_clones.iter().map(Option::is_some).collect();
    direct.set_parallel(&admitted);
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
            writeln!(
                output,
                "static const kiln_shape *{symbol}_fn_{index}_recv_shape[4];\nstatic uint32_t {symbol}_fn_{index}_recv_slot[4][{columns}];\nstatic const kiln_shape *{symbol}_fn_{index}_typed_recv_shape[4];\nstatic uint32_t {symbol}_fn_{index}_typed_recv_slot[4][{columns}];"
            )
            .unwrap();
        }
    }
    for (index, &has_fast) in fast.iter().enumerate() {
        if reachable.is_some_and(|set| !set.contains(&index)) {
            continue;
        }
        writeln!(
            output,
            "static inline int {symbol}_fn_{index}(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues, const kiln_val *receiver_ptr, const kiln_val *args, size_t argc);"
        )
        .unwrap();
        if has_fast {
            writeln!(
                output,
                "static inline int {symbol}_fn_{index}_f(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues, const kiln_val *receiver_ptr, const kiln_val *args, size_t argc);"
            )
            .unwrap();
        }
        if direct.has_typed_entry(index as u32) {
            let (result, params) = emit_graph::typed_signature(
                &program.functions[index],
                direct.entry.typed_returns[index],
                direct.entry.typed_needs[index],
            );
            writeln!(
                output,
                "static inline {result} {symbol}_fn_{index}_t({params});"
            )
            .unwrap();
            if direct.parallel_chunk(index as u32) {
                writeln!(
                    output,
                    "static void {symbol}_pmap_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);\n\
                     static void {symbol}_pmap_f64_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);\n\
                     static void {symbol}_pmap_f32_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);\n\
                     static void {symbol}_pmap_i32_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);\n\
                     static void {symbol}_pmap_u32_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);\n\
                     static void {symbol}_pmap_i16_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);\n\
                     static void {symbol}_pmap_u16_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);\n\
                     static void {symbol}_pmap_i8_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);\n\
                     static void {symbol}_pmap_u8_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);\n\
                     static void {symbol}_pmap_u8c_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);\n\
                     static void {symbol}_pmap_typed_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx);"
                )
                .unwrap();
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
    for (index, proto) in program.functions.iter().enumerate() {
        if reachable.is_some_and(|set| !set.contains(&index)) {
            continue;
        }
        let scalar_proto = !proto_produces_references(proto, program);
        let func_profile = policy.type_profile.as_ref().and_then(|p| p.function(index));
        emit_graph::emit_function_from_graph_with_profile(
            output,
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
        );
        if let Some(typed) = &direct.typed[index] {
            emit_typed(output, index, typed, true);
            if let Some(clone) = &parallel_clones[index] {
                output.push_str(clone);
                emit_graph::emit_parallel_chunk(output, symbol, index, proto.parameter_count);
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
            output.push_str(&clone);
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
}

/// Whether this body's parameter binding runs during the call rather than at the first
/// resumption.
///
/// Only a suspending body has the two halves apart at all: an ordinary call runs them together
/// and never needs the seam. An async *function* is the one suspending shape that must keep them
/// together too -- its instantiation failure rejects rather than throwing, and running the
/// prologue as the first thing its driver does is already exactly that.
fn binds_at_call(proto: &FunctionProto) -> bool {
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
    generic_only: bool,
    speculate: Option<bool>,
    extra_entry_slots: Option<&HashSet<u32>>,
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
    let publishes_frame = compute_publishes_frame(program);
    let direct = direct::DirectPlan::plan(
        program,
        &graphs,
        &entry_graph,
        &publishes_frame,
        compact,
        generic_only,
        speculate,
    );
    Ok((graphs, body, entry_graph, direct))
}

/// The unit's entry point: opened with the embedding signature, its locals declared and the
/// imports bound into them, then the body from its graph.
///
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
) -> Result<(), EmitError> {
    if body.asynchronous {
        let entry_name = format!("{unit_symbol}_entry_name");
        render_static_string(output, &entry_name, &kiln_ir::StringUnits::from("<entry>"));
        emit_graph::emit_async_entry_body(
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
        );

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
            "    (void)kiln_realm_gc_register_roots({unit_symbol}_gc_roots, NULL, 0);"
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

        writeln!(
            output,
            "    for (size_t slot = 0; slot < (size_t){locals}; ++slot) locals[slot] = KILN_VAL_UNDEFINED;"
        )
        .unwrap();

        if let Some((module_prog, _)) = module {
            let boxed_imports = slots_read_as_cells(&program.code, &program.functions);
            for (index, binding) in module_prog.imports.iter().enumerate() {
                let slot = binding.local_slot;
                writeln!(
                    output,
                    "    locals[{slot}] = kiln_val_from_boundary(imports[{index}]);"
                )
                .unwrap();
                if boxed_imports.contains(&slot) {
                    writeln!(
                        output,
                        "    if ((locals[{slot}] & KILN_TAG_MASK) != KILN_TAG_CELL_PREFIX && kiln_c99_val_make_cell(&locals[{slot}]) != KILN_C99_OK) return KILN_C99_OUT_OF_MEMORY;"
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
        return Ok(());
    }

    let callables = produces_references(program) || (policy.compact && module.is_some());
    let entry_symbol = if callables {
        format!("{unit_symbol}_body")
    } else {
        unit_symbol.to_owned()
    };
    let module_scope = uses_module_scope(program);
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
        "    (void)kiln_realm_gc_register_roots({unit_symbol}_gc_roots, NULL, 0);"
    )
    .unwrap();
    render_key_prologue(output, unit_symbol, keyed);
    if policy.type_profile_record {
        writeln!(output, "    (void)atexit({unit_symbol}_dump_type_profile);").unwrap();
    }
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
    let locals = usize::try_from(program.local_count).unwrap().max(1);
    if module_scope {
        writeln!(output, "    kiln_val *locals = {unit_symbol}_globals;").unwrap();
    } else {
        writeln!(output, "    kiln_val locals[{locals}];").unwrap();
    }
    output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
    let mut array_slots: HashSet<u32> = if module_scope {
        (0..program.local_count).collect()
    } else {
        emit_graph::array_slots(graph, program)
    };
    let mut exports: Option<&[kiln_ir::ExportBinding]> = None;
    let mut commonjs = false;
    match module {
        None => output.push_str("    kiln_c99_reset_diagnostics();\n"),
        Some((module, is_commonjs)) => {
            commonjs = is_commonjs;
            exports = Some(&module.exports);
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
            array_slots.extend(module.imports.iter().map(|binding| binding.local_slot));
            array_slots.extend(module.exports.iter().map(|binding| binding.local_slot));
            if is_commonjs {
                // `module`, whose `exports` is the unit's one export.
                array_slots.insert(2);
            }
        }
    }
    writeln!(
        output,
        "    for (size_t slot = 0; slot < (size_t){locals}; ++slot) locals[slot] = KILN_VAL_UNDEFINED;"
    )
    .unwrap();
    if let Some((module, _)) = module {
        let boxed_imports = slots_read_as_cells(&program.code, &program.functions);
        for (index, binding) in module.imports.iter().enumerate() {
            let slot = binding.local_slot;
            writeln!(
                output,
                "    locals[{slot}] = kiln_val_from_boundary(imports[{index}]);"
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
    emit_graph::emit_entry_from_graph_with_profile(
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
    );
    output.push_str("}\n");
    render_publish_exports(output, public_symbol, unit_symbol, module, module_scope);
    Ok(())
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
                "void {public_symbol}_publish_exports(kiln_value *exports) {{\n    if (exports == NULL) return;"
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
    let units: Vec<u16> = value.units().collect();
    let latin1 = units.iter().all(|unit| *unit <= 0xff);
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
            "static const kiln_string {name} = {{ {}, KILN_STRING_LATIN1, {name}_data }};",
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
            "static const kiln_string {name} = {{ {}, KILN_STRING_UTF16, {name}_data }};",
            units.len()
        )
        .unwrap();
    }
}

fn render_constant_declarations(output: &mut String, program: &Program, symbol: &str) {
    for (index, constant) in program.constants.iter().enumerate() {
        match constant {
            Constant::String(value) => {
                let name = format!("{symbol}_string_{index}");
                render_static_string(output, &name, value);
                writeln!(output, "#define {symbol}_constant_{index} (&{name})").unwrap();
            }
            Constant::BigInt(digits) => {
                // Headed like every other string constant: the digits are ASCII, so the
                // static lands Latin1 and the keyed parse reads its payload in place.
                let name = format!("{symbol}_bigint_digits_{index}");
                render_static_string(output, &name, &kiln_ir::StringUnits::from(digits.as_str()));
                writeln!(output, "#define {symbol}_constant_{index} (&{name})").unwrap();
                writeln!(output, "static void *{symbol}_bigint_{index} = NULL;").unwrap();
            }
            Constant::Undefined | Constant::Null | Constant::Boolean(_) | Constant::Number(_) => {}
        }
    }
    for (index, function) in program.functions.iter().enumerate() {
        if let Some(name) = function.effective_name_units() {
            let symbol_name = format!("{symbol}_function_name_{index}");
            render_static_string(output, &symbol_name, &name);
        }
    }
}

fn render_regex_matchers(output: &mut String, program: &Program, symbol: &str) {
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
            crate::regex::emit_regex_matcher(output, &prefix, &compiled);
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
            | Instruction::DeleteProperty(name)
            | Instruction::LoadSuperProperty(name)
            | Instruction::SetPropertyStrict { name, .. }
            | Instruction::SetSuperPropertyStrict { name, .. }
            | Instruction::DeletePropertyStrict { name, .. }
            | Instruction::CallMethod { name, .. } => Some(*name),
            _ => None,
        }
    }
    let mut keys: Vec<u32> = program
        .code
        .iter()
        .chain(program.functions.iter().flat_map(|proto| proto.code.iter()))
        .filter_map(key_of)
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// One canonical pointer per key the unit names, filled at the first entry into a realm and
/// cleared by the unit's `KILN_GC_PHASE_RESET` when that realm is destroyed. Interned as static:
/// the constant table outlives every heap, so the entry is pinned and nothing is copied.
fn render_key_table(output: &mut String, program: &Program, symbol: &str, keys: &[u32]) {
    if keys.is_empty() {
        return;
    }
    for index in keys {
        writeln!(output, "static const kiln_string *{symbol}_key_{index};").unwrap();
    }
    writeln!(output, "static int {symbol}_keys_ready;").unwrap();
    writeln!(output, "static int {symbol}_intern_keys(void) {{").unwrap();
    for index in keys {
        let Constant::String(_) = &program.constants[*index as usize] else {
            unreachable!("verification rejects a non-string property name before emission")
        };
        // The key store is canonical headed keys now: intern the constant's header
        // directly, in either width. The Latin1 bridge and its UTF-16 refusal are gone.
        writeln!(
            output,
            "    if (({symbol}_key_{index} = kiln_c99_intern_static_key(&{symbol}_string_{index})) == NULL) return KILN_C99_OUT_OF_MEMORY;"
        )
        .unwrap();
    }
    writeln!(
        output,
        "    {symbol}_keys_ready = 1;\n    return KILN_C99_OK;\n}}"
    )
    .unwrap();
}

/// The line a unit's entry point runs to canonicalize its keys before the first access. A key
/// that cannot be interned fails the entry: no site ever runs with a pointer that is not
/// canonical.
fn render_key_prologue(output: &mut String, symbol: &str, keyed: bool) {
    if keyed {
        writeln!(
            output,
            "    if (!{symbol}_keys_ready) {{ int keyed = {symbol}_intern_keys(); if (keyed != KILN_C99_OK) return keyed; }}"
        )
        .unwrap();
    }
}

/// The root function this unit registers with the realm (ADR 0041): its module-scope slots,
/// when it has them, and the prelude statics every unit owns a copy of.
fn render_template_sites(output: &mut String, program: &Program, symbol: &str) {
    if program.template_sites.is_empty() {
        return;
    }
    for (site_idx, site) in program.template_sites.iter().enumerate() {
        for (raw_idx, raw) in site.raw.iter().enumerate() {
            let name = format!("{symbol}_site_{site_idx}_raw_{raw_idx}");
            render_static_string(output, &name, raw);
        }
        for (cooked_idx, cooked) in site.cooked.iter().enumerate() {
            if let Some(text) = cooked {
                let name = format!("{symbol}_site_{site_idx}_cooked_{cooked_idx}");
                render_static_string(output, &name, text);
            }
        }
    }
    writeln!(
        output,
        "static kiln_val {symbol}_template_sites[{}];",
        program.template_sites.len()
    )
    .unwrap();
    writeln!(
        output,
        "static int {symbol}_materialize_template_site(size_t site) {{"
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
                "        raw_arr->elements[{raw_idx}] = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &{symbol}_site_{site_idx}_raw_{raw_idx});"
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
                    "        cooked_arr->elements[{cooked_idx}] = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &{symbol}_site_{site_idx}_cooked_{cooked_idx});"
                )
                .unwrap();
            } else {
                writeln!(
                    output,
                    "        cooked_arr->elements[{cooked_idx}] = KILN_VAL_UNDEFINED;"
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
        writeln!(output, "        raw_slot->value = raw_built;").unwrap();
        writeln!(output, "        raw_slot->writable = 0;").unwrap();
        writeln!(output, "        raw_slot->enumerable = 0;").unwrap();
        writeln!(output, "        raw_slot->configurable = 0;").unwrap();
        writeln!(output, "        raw_slot->accessor = 0;").unwrap();
        writeln!(
            output,
            "        kiln_c99_object_of(props)->read_only_seen = 1;"
        )
        .unwrap();
        writeln!(output, "        cooked_arr->properties = props;").unwrap();
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

fn render_unit_roots(
    output: &mut String,
    symbol: &str,
    module_scope: bool,
    local_count: u32,
    keyed: bool,
    template_sites_count: usize,
) {
    writeln!(
        output,
        "static void {symbol}_gc_roots(void *heap, void *context, int phase) {{"
    )
    .unwrap();
    if keyed {
        // The canonical pointers belong to the realm's intern table, which dies with the realm.
        writeln!(
            output,
            "    if (phase == KILN_GC_PHASE_RESET) {symbol}_keys_ready = 0;"
        )
        .unwrap();
    }
    if template_sites_count > 0 {
        writeln!(
            output,
            "    if (phase == KILN_GC_PHASE_MARK) kiln_c99_gc_mark_vals((struct kiln_gc_heap *)heap, {symbol}_template_sites, {template_sites_count});"
        )
        .unwrap();
        writeln!(
            output,
            "    if (phase == KILN_GC_PHASE_RESET) memset({symbol}_template_sites, 0, sizeof({symbol}_template_sites));"
        )
        .unwrap();
    }
    if module_scope {
        writeln!(
            output,
            "    if (phase == KILN_GC_PHASE_MARK) kiln_c99_gc_mark_vals((struct kiln_gc_heap *)heap, {symbol}_globals, {});",
            usize::try_from(local_count).unwrap().max(1)
        )
        .unwrap();
    }
    output.push_str("    kiln_c99_gc_unit_roots(heap, context, phase);\n}\n");
}

fn render_scalar_realm_wrapper(output: &mut String, symbol: &str) {
    writeln!(
        output,
        r#"
typedef struct {symbol}_realm_context {{
    kiln_host *host;
    kiln_value *result;
}} {symbol}_realm_context;

static int {symbol}_gc_result_entry(void *result) {{
    return kiln_realm_gc_register_roots(kiln_c99_gc_result_roots, result,
                                        KILN_GC_ROOT_SURVIVES_TEARDOWN);
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

fn render_module_realm_wrapper(output: &mut String, symbol: &str, unit_symbol: &str) {
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

static int {unit_symbol}_gc_result_entry(void *result) {{
    return kiln_realm_gc_register_roots(kiln_c99_gc_result_roots, result,
                                        KILN_GC_ROOT_SURVIVES_TEARDOWN);
}}

static int {unit_symbol}_realm_entry(void *opaque) {{
    {unit_symbol}_realm_context *context = ({unit_symbol}_realm_context *)opaque;
    int status;
    if (context == NULL) return KILN_C99_INVALID_ARGUMENT;
    status = {unit_symbol}_body(context->host, context->result, context->imports,
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
        status = {unit_symbol}_body(host, result, imports, import_count, exports, export_count);
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
    int status;
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
    status = kiln_c99_new_object(&object);
    if (status != KILN_C99_OK) {{
        free(order);
        return status;
    }}
    data = kiln_c99_object_of(object);
    data->shape = NULL;
    data->prototype = kiln_c99_null();
    for (index = 0; index < count; ++index) {{
        size_t source = order[index];
        kiln_property *slot;
        kiln_value value = values[source];
        while (value.tag == KILN_VALUE_CELL) {{
            if (value.bits == 0) {{
                status = KILN_C99_CONTROL_ERROR;
                goto namespace_failure;
            }}
            value = kiln_val_to_boundary(*kiln_c99_cell_of(value));
        }}
        status = kiln_c99_reserve((void **)&data->properties, &data->capacity, data->count + 1,
                                  sizeof *data->properties);
        if (status != KILN_C99_OK) {{
            goto namespace_failure;
        }}
        /* Namespace export keys stay in the linker's lexical order. Ordinary object insertion
         * would move integer-looking names to numeric order, which is not the namespace exotic
         * object's [[OwnPropertyKeys]] rule. */
        slot = &data->properties[data->count];
        /* Canonical like every stored key (ADR 0047). Copied rather than registered as static:
         * the launcher's names are its own, and this seam promises nothing about their life. */
        slot->key = kiln_c99_intern(names[source], name_lengths[source]);
        if (slot->key == NULL) {{
            status = KILN_C99_OUT_OF_MEMORY;
            goto namespace_failure;
        }}
        data->count += 1;
        slot->key_len = name_lengths[source];
        slot->value = value;
        slot->symbol_hidden = 0;
        slot->writable = 1;
        slot->enumerable = 1;
        slot->configurable = 0;
        slot->accessor = 0;
        slot->refused = 0;
        slot->getter = kiln_c99_undefined();
        slot->setter = kiln_c99_undefined();
    }}
    {{
        kiln_property *tag;
        status = kiln_c99_own_slot(object, "@@toStringTag", 13, &tag);
        if (status == KILN_C99_OK) {{
            tag->value = kiln_c99_copy_text("Module", 6);
            if (tag->value.tag != KILN_VALUE_STRING) status = KILN_C99_OUT_OF_MEMORY;
            tag->writable = 0;
            kiln_c99_object_of(object)->read_only_seen = 1;
            tag->enumerable = 0;
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
    if (data != NULL) {{
        for (index = 0; index < data->count; ++index) {{
            if (data->properties[index].key_len == (size_t)13 &&
                memcmp(data->properties[index].key, "@@toStringTag", (size_t)13) == 0 &&
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

fn render(
    program: &Program,
    program_hash: u64,
    symbol: &str,
    policy: &EmissionPolicy,
) -> Result<String, EmitError> {
    let plan = validate_program(program).expect("render called after validation");
    let mut compacted;
    let program = if policy.compact {
        compacted = program.clone();
        compact_closure_source(&mut compacted, None);
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
    render_constant_declarations(&mut output, program, symbol);
    if !policy.compact {
        render_regex_matchers(&mut output, program, symbol);
    }
    let keys = property_key_constants(program);
    render_key_table(&mut output, program, symbol, &keys);
    render_template_sites(&mut output, program, symbol);
    if uses_module_scope(program) {
        writeln!(
            output,
            "static kiln_val {symbol}_globals[{}];",
            usize::try_from(program.local_count).unwrap().max(1)
        )
        .unwrap();
    }
    render_unit_roots(
        &mut output,
        symbol,
        uses_module_scope(program),
        program.local_count,
        !keys.is_empty(),
        program.template_sites.len(),
    );
    let extra_entry_slots = if uses_module_scope(program) {
        Some((0..program.local_count).collect::<HashSet<u32>>())
    } else {
        None
    };
    let (graphs, body, entry_graph, mut direct) = plan_unit(
        program,
        symbol,
        policy.compact,
        policy.generic_only,
        policy.speculate,
        extra_entry_slots.as_ref(),
    )?;
    if program.has_dynamic_import() {
        output.push_str(
            "__attribute__((weak)) int kiln_graph_dynamic_import(kiln_host *host, const char *importer, kiln_value specifier, kiln_value options, kiln_value *promise, int depth, kiln_value *thrown) {\n    (void)importer; (void)specifier; (void)options;\n    kiln_value err = kiln_c99_undefined();\n    (void)kiln_c99_throw_error(&err, KILN_C99_BUILTIN_TYPE_ERROR, \"no module loader is installed\");\n    return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);\n}\n",
        );
    }
    render_functions(
        &mut output,
        program,
        &plan,
        symbol,
        symbol,
        &strict_functions,
        &graphs,
        &mut direct,
        policy,
        None,
    );
    render_entry(
        &mut output,
        program,
        &plan,
        symbol,
        symbol,
        symbol,
        &strict_functions,
        !keys.is_empty(),
        None,
        &body,
        &entry_graph,
        &direct.entry,
        policy,
    )?;
    if callables {
        render_scalar_realm_wrapper(&mut output, symbol);
    }
    Ok(output)
}

fn function_parent_candidates(program: &Program) -> Vec<Vec<Option<usize>>> {
    let mut parents = vec![Vec::<Option<usize>>::new(); program.functions.len()];
    for instruction in &program.code {
        if let Instruction::MakeFunction(index) = instruction
            && let Some(found) = parents.get_mut(*index as usize)
        {
            found.push(None);
        }
    }
    for (parent, function) in program.functions.iter().enumerate() {
        for instruction in &function.code {
            if let Instruction::MakeFunction(index) = instruction
                && let Some(found) = parents.get_mut(*index as usize)
            {
                found.push(Some(parent));
            }
        }
    }
    for found in &mut parents {
        found.sort_unstable();
        found.dedup();
    }
    parents
}

fn module_upvalue_slots_with_parents(
    program: &Program,
    parent_candidates: &[Vec<Option<usize>>],
) -> Vec<Vec<Option<u32>>> {
    let parents = parent_candidates
        .iter()
        .map(|found| (found.len() == 1).then(|| found[0]))
        .collect::<Vec<_>>();
    let mut module_upvalues = program
        .functions
        .iter()
        .map(|function| vec![None; function.upvalues.len()])
        .collect::<Vec<Vec<Option<u32>>>>();
    for _ in 0..program.functions.len().max(1) {
        let mut changed = false;
        for (index, function) in program.functions.iter().enumerate() {
            for (upvalue_index, upvalue) in function.upvalues.iter().enumerate() {
                let resolved = match (parents[index], upvalue) {
                    (
                        Some(None),
                        Upvalue::ParentLocal(slot) | Upvalue::NamedParentLocal { slot, .. },
                    ) => Some(*slot),
                    (
                        Some(Some(parent)),
                        Upvalue::ParentUpvalue(parent_index)
                        | Upvalue::NamedParentUpvalue {
                            index: parent_index,
                            ..
                        },
                    ) => module_upvalues
                        .get(parent)
                        .and_then(|slots| slots.get(*parent_index as usize))
                        .copied()
                        .flatten(),
                    _ => None,
                };
                if resolved.is_some() && module_upvalues[index][upvalue_index] != resolved {
                    module_upvalues[index][upvalue_index] = resolved;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    module_upvalues
}

fn captured_slot_owner(
    program: &Program,
    parents: &[Vec<Option<usize>>],
    function: usize,
    upvalue: usize,
) -> Option<(Option<usize>, u32)> {
    let mut function = function;
    let mut upvalue = upvalue;
    let mut seen = HashSet::new();
    loop {
        if !seen.insert((function, upvalue)) {
            return None;
        }
        let parent = *parents.get(function)?.as_slice().first()?;
        if parents[function].len() != 1 {
            return None;
        }
        match *program.functions.get(function)?.upvalues.get(upvalue)? {
            Upvalue::ParentLocal(slot) | Upvalue::NamedParentLocal { slot, .. } => {
                return Some((parent, slot));
            }
            Upvalue::ParentUpvalue(index) | Upvalue::NamedParentUpvalue { index, .. } => {
                function = parent?;
                upvalue = index as usize;
            }
        }
    }
}

fn stored_declaration(code: &[Instruction], pc: usize) -> Option<(u32, bool)> {
    match code.get(pc + 1) {
        Some(Instruction::StoreLocal(slot)) | Some(Instruction::StoreCell(slot)) => {
            Some((*slot, false))
        }
        Some(Instruction::StoreGlobal(slot)) => Some((*slot, true)),
        _ => None,
    }
}

/// Functions a closed-world module can instantiate, and the dead declaration stores that made
/// the others unreachable. The same analysis runs in every reachable lexical scope: bundled
/// modules commonly put thousands of helper declarations inside one live wrapper, and treating
/// every nested `MakeFunction` as live defeats closed-world linking almost completely.
///
/// Only the compiler's simplest declaration shape, `MakeFunction; Store*`, is removable. Class
/// methods and functions installed into objects remain live. A hand-authored program whose
/// capture parent is ambiguous falls back to the complete function table.
///
/// `pub(crate)` so the facts analysis can read retained code for member-name demand.
pub(crate) fn closed_world_function_liveness(
    program: &Program,
    demanded: &BTreeSet<u32>,
    direct_eval: bool,
    declarations: &[RemovableDeclaration],
) -> (HashSet<usize>, Vec<HashSet<u32>>, BTreeSet<u32>) {
    if direct_eval {
        return (
            (0..program.functions.len()).collect(),
            vec![HashSet::new(); program.functions.len() + 1],
            (0..program.local_count).collect(),
        );
    }
    let parents = function_parent_candidates(program);
    let mut module_live = demanded.clone();
    let mut function_live = vec![BTreeSet::<u32>::new(); program.functions.len()];
    let mut reachable = HashSet::new();
    let mut declaration_at_pc = vec![None; program.code.len()];
    for (index, declaration) in declarations.iter().enumerate() {
        let start = declaration.pc_start as usize;
        let end = declaration.pc_end as usize;
        if start > end || end > declaration_at_pc.len() {
            continue;
        }
        for owner in &mut declaration_at_pc[start..end] {
            *owner = Some(index);
        }
    }
    let mut active_declarations = vec![false; declarations.len()];

    loop {
        let mut changed = false;
        for (index, declaration) in declarations.iter().enumerate() {
            if !active_declarations[index]
                && declaration
                    .local_slots
                    .iter()
                    .any(|slot| module_live.contains(slot))
            {
                active_declarations[index] = true;
                changed = true;
            }
        }
        let live_owners = std::iter::once(None::<usize>)
            .chain(reachable.iter().copied().map(Some))
            .collect::<Vec<_>>();
        for owner in live_owners {
            let code = match owner {
                None => program.code.as_slice(),
                Some(index) => program.functions[index].code.as_slice(),
            };
            for (pc, instruction) in code.iter().enumerate() {
                if owner.is_none()
                    && declaration_at_pc[pc].is_some_and(|index| !active_declarations[index])
                {
                    continue;
                }
                match instruction {
                    Instruction::LoadGlobal(slot) => changed |= module_live.insert(*slot),
                    Instruction::LoadLocal(slot) | Instruction::LoadCell(slot) => match owner {
                        None => changed |= module_live.insert(*slot),
                        Some(index) => changed |= function_live[index].insert(*slot),
                    },
                    _ => {}
                }
            }
            for (pc, instruction) in code.iter().enumerate() {
                if owner.is_none()
                    && declaration_at_pc[pc].is_some_and(|index| !active_declarations[index])
                {
                    continue;
                }
                let Instruction::MakeFunction(index) = instruction else {
                    continue;
                };
                let target = *index as usize;
                let is_live = match stored_declaration(code, pc) {
                    Some((slot, true)) => module_live.contains(&slot),
                    Some((slot, false)) => match owner {
                        None => module_live.contains(&slot),
                        Some(index) => function_live[index].contains(&slot),
                    },
                    None => true,
                };
                if is_live {
                    changed |= reachable.insert(target);
                }
            }
        }

        let reachable_now = reachable.iter().copied().collect::<Vec<_>>();
        for index in reachable_now {
            let Some(function) = program.functions.get(index) else {
                continue;
            };
            for upvalue in 0..function.upvalues.len() {
                let Some((owner, slot)) = captured_slot_owner(program, &parents, index, upvalue)
                else {
                    return (
                        (0..program.functions.len()).collect(),
                        vec![HashSet::new(); program.functions.len() + 1],
                        (0..program.local_count).collect(),
                    );
                };
                match owner {
                    None => changed |= module_live.insert(slot),
                    Some(parent) => changed |= function_live[parent].insert(slot),
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut dead_pcs = vec![HashSet::new(); program.functions.len() + 1];
    for (index, declaration) in declarations.iter().enumerate() {
        if !active_declarations[index] {
            dead_pcs[0].extend(declaration.pc_start..declaration.pc_end);
        }
    }
    for (owner, code) in std::iter::once((None, program.code.as_slice())).chain(
        program
            .functions
            .iter()
            .enumerate()
            .filter(|(index, _)| reachable.contains(index))
            .map(|(index, function)| (Some(index), function.code.as_slice())),
    ) {
        for (pc, instruction) in code.iter().enumerate() {
            let Instruction::MakeFunction(index) = instruction else {
                continue;
            };
            let dead = match stored_declaration(code, pc) {
                Some((slot, true)) => !module_live.contains(&slot),
                Some((slot, false)) => match owner {
                    None => !module_live.contains(&slot),
                    Some(index) => !function_live[index].contains(&slot),
                },
                None => false,
            };
            if dead && !reachable.contains(&(*index as usize)) {
                let scope = owner.map_or(0, |index| index + 1);
                dead_pcs[scope].insert(pc as u32);
                dead_pcs[scope].insert((pc + 1) as u32);
            }
        }
    }
    (reachable, dead_pcs, module_live)
}

/// Module slots read by evaluation or by a function reachable from a demanded export.
///
/// This is the linker-facing half of the same conservative analysis used during emission.
pub fn closed_world_module_live_slots(
    program: &Program,
    demanded: &BTreeSet<u32>,
    direct_eval: bool,
) -> BTreeSet<u32> {
    closed_world_function_liveness(program, demanded, direct_eval, &[]).2
}

/// [`closed_world_module_live_slots`] with compiler-recorded pure declaration boundaries.
pub fn closed_world_module_live_slots_with_declarations(
    program: &Program,
    demanded: &BTreeSet<u32>,
    direct_eval: bool,
    declarations: &[RemovableDeclaration],
) -> BTreeSet<u32> {
    closed_world_function_liveness(program, demanded, direct_eval, declarations).2
}

/// Instruction and function counts after the same declaration liveness used by C emission.
/// Without a closed-world export demand, conservatively count the entire module. This is an
/// IR-only query: callers can choose specialization before constructing any SSA graphs.
pub fn retained_module_size(
    module: &ModuleProgram,
    demanded: Option<&BTreeSet<u32>>,
) -> (usize, usize) {
    let program = &module.program;
    let Some(demanded) = demanded else {
        return (
            program.code.len()
                + program
                    .functions
                    .iter()
                    .map(|f| f.code.len())
                    .sum::<usize>(),
            program.functions.len(),
        );
    };
    let (reachable, dead_pcs, _) = closed_world_function_liveness(
        program,
        demanded,
        module.link_facts.direct_eval,
        &module.link_facts.removable_declarations,
    );
    let instructions = program.code.len() - dead_pcs[0].len()
        + reachable
            .iter()
            .map(|&index| program.functions[index].code.len() - dead_pcs[index + 1].len())
            .sum::<usize>();
    (instructions, reachable.len())
}

/// Instructions retained by the same declaration/function liveness used during C emission.
/// Consumers planning separately linked runtime engines must inspect this view rather than every
/// function prototype in the source module, or dead library code keeps unrelated engines alive.
pub fn retained_module_instructions<'a>(
    module: &'a ModuleProgram,
    demanded: Option<&BTreeSet<u32>>,
) -> Vec<&'a Instruction> {
    let program = &module.program;
    let Some(demanded) = demanded else {
        return program
            .code
            .iter()
            .chain(
                program
                    .functions
                    .iter()
                    .flat_map(|function| function.code.iter()),
            )
            .collect();
    };
    let (reachable, dead_pcs, _) = closed_world_function_liveness(
        program,
        demanded,
        module.link_facts.direct_eval,
        &module.link_facts.removable_declarations,
    );
    let mut retained = program
        .code
        .iter()
        .enumerate()
        .filter(|(pc, _)| !dead_pcs[0].contains(&(*pc as u32)))
        .map(|(_, instruction)| instruction)
        .collect::<Vec<_>>();
    for index in reachable {
        retained.extend(
            program.functions[index]
                .code
                .iter()
                .enumerate()
                .filter(|(pc, _)| !dead_pcs[index + 1].contains(&(*pc as u32)))
                .map(|(_, instruction)| instruction),
        );
    }
    retained
}

/// Statically named members read from one module namespace slot, or `None` when the namespace
/// escapes and therefore needs its complete export surface.
///
/// The value graph supplies the stack provenance that bytecode alone does not: arguments may be
/// evaluated between loading a namespace and calling its method. Aliases used only to carry the
/// same value across TDZ checks, locals, or control-flow joins remain inspectable. Any other use
/// is deliberately the conservative answer.
pub fn closed_world_namespace_members(
    program: &Program,
    slot: u32,
    direct_eval: bool,
) -> Option<BTreeSet<String>> {
    if direct_eval {
        return None;
    }
    let parent_candidates = function_parent_candidates(program);
    // A compiler-produced function has one lexical creation site. Hand-authored IR can reuse a
    // function prototype under multiple parents; an upvalue in that shape has no single module
    // provenance, so a partial namespace answer would be unsound.
    if program
        .functions
        .iter()
        .enumerate()
        .any(|(index, function)| {
            parent_candidates[index].len() > 1 && !function.upvalues.is_empty()
        })
    {
        return None;
    }
    let module_upvalues = module_upvalue_slots_with_parents(program, &parent_candidates);
    let entry = graph_for(
        &entry_proto(program),
        program,
        "the module body",
        "namespace liveness",
    )
    .ok()?;
    let functions = program
        .functions
        .iter()
        .enumerate()
        .map(|(index, function)| {
            graph_for(
                function,
                program,
                &format!("function {index}"),
                "namespace liveness",
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let mut members = BTreeSet::new();
    for (graph, function_index) in std::iter::once((&entry, None)).chain(
        functions
            .iter()
            .enumerate()
            .map(|(index, graph)| (graph, Some(index))),
    ) {
        let defs = graph
            .blocks
            .iter()
            .flat_map(|block| block.operations.iter().map(|(value, op)| (*value, op)))
            .collect::<std::collections::HashMap<_, _>>();
        let roots = graph
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .filter_map(|(value, op)| match op {
                GraphOp::LoadGlobal(found) if *found == slot => Some(*value),
                GraphOp::LoadLocal(found) | GraphOp::LoadCell(found)
                    if function_index.is_none() && *found == slot =>
                {
                    Some(*value)
                }
                GraphOp::LoadUpvalue(index)
                    if function_index.is_some_and(|function| {
                        module_upvalues[function]
                            .get(*index as usize)
                            .is_some_and(|found| *found == Some(slot))
                    }) =>
                {
                    Some(*value)
                }
                _ => None,
            })
            .collect::<HashSet<_>>();
        if roots.is_empty() {
            continue;
        }
        let is_namespace = |value| roots.contains(&graph::resolve_root_alias(value, &defs));
        for block in &graph.blocks {
            for (_, op) in &block.operations {
                let namespace_operands = op
                    .operands()
                    .into_iter()
                    .filter(|value| is_namespace(*value))
                    .collect::<Vec<_>>();
                if namespace_operands.is_empty() {
                    continue;
                }
                match op {
                    GraphOp::GetProperty { object, name, .. }
                    | GraphOp::CallMethod { object, name, .. }
                        if is_namespace(*object) =>
                    {
                        let Some(Constant::String(name)) = program.constants.get(*name as usize)
                        else {
                            return None;
                        };
                        let Some(name) = name.to_scalar_string().ok() else {
                            return None;
                        };
                        members.insert(name);
                    }
                    GraphOp::CheckTdz { .. } | GraphOp::StoreLocal { .. } | GraphOp::Phi { .. } => {
                    }
                    _ => return None,
                }
            }
            let escapes = match &block.terminator {
                Terminator::Branch { condition, .. }
                | Terminator::BranchNotNullish { condition, .. } => is_namespace(*condition),
                Terminator::Return(Some(value)) | Terminator::Throw(value) => is_namespace(*value),
                Terminator::Suspend { value, spilled, .. } => {
                    is_namespace(*value) || spilled.iter().any(|value| is_namespace(*value))
                }
                Terminator::Jump(_) | Terminator::Return(None) | Terminator::Unreachable => false,
            };
            if escapes {
                return None;
            }
        }
    }
    Some(members)
}

fn render_module(
    module: &ModuleProgram,
    program_hash: u64,
    symbol: &str,
    policy: &EmissionPolicy,
    commonjs: bool,
    diagnostic_site: &str,
) -> Result<String, EmitError> {
    let program = &module.program;
    // Compact graph modules are each compiled as their own translation unit. Private identifiers
    // therefore do not need the link-visible content hash; retain it only on exported adapters.
    let unit_symbol = if policy.compact { "ku" } else { symbol };
    let plan = validate_program(program).expect("render called after validation");
    let callables = produces_references(program) || policy.compact;
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
        output.push_str("#define KU_STATUS(PC, OP, UNWIND) do { if (status == KILN_C99_THROWN) { UNWIND; } if (status == KILN_C99_CONTROL_ERROR) { kiln_c99_note_refusal(ku_site, (size_t)(PC), (OP), (size_t)kiln_c99_diagnostic_active_depth); goto kiln_refusal; } if (status != KILN_C99_OK) return status; } while (0)\n");
        output.push_str("#define KU_LOAD_CELL(DST, CELL, THROWN, PC, OP, UNWIND) do { const kiln_val *ku_cell = (const kiln_val *)KILN_VAL_PTR(CELL); int status = kiln_c99_val_check_tdz(*ku_cell, (THROWN)); KU_STATUS((PC), (OP), UNWIND); (DST) = *ku_cell; } while (0)\n");
        output.push_str("#define KU_BINARY(DST, CODE, LEFT, RIGHT, PC, OP, UNWIND) do { int status = KILN_C99_OK; (DST) = kiln_c99_val_binary((CODE), host, (LEFT), (RIGHT), &status, depth, thrown); KU_STATUS((PC), (OP), UNWIND); } while (0)\n");
        output.push_str("#define KU_READ_PROPERTY(DST, OBJECT, KEY, KEY_LEN, SITE, PC, OP, UNWIND) do { int status = kiln_c99_val_read_property_key(host, (OBJECT), (KEY), depth, thrown, (SITE), &(DST)); if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_property_refusal(kiln_val_to_boundary(OBJECT)); KU_STATUS((PC), (OP), UNWIND); } while (0)\n");
        COMPACT_DIAGNOSTIC_SITE
    } else {
        diagnostic_site
    };
    render_constant_declarations(&mut output, program, unit_symbol);
    if !policy.compact {
        render_regex_matchers(&mut output, program, unit_symbol);
    }
    let keys = property_key_constants(program);
    render_key_table(&mut output, program, unit_symbol, &keys);
    render_template_sites(&mut output, program, unit_symbol);
    if uses_module_scope(program) {
        writeln!(
            output,
            "static kiln_val {unit_symbol}_globals[{}];",
            usize::try_from(program.local_count).unwrap().max(1)
        )
        .unwrap();
    }
    render_unit_roots(
        &mut output,
        unit_symbol,
        uses_module_scope(program),
        program.local_count,
        !keys.is_empty(),
        program.template_sites.len(),
    );
    let liveness = policy.live_module_slots.as_ref().map(|demanded| {
        closed_world_function_liveness(
            program,
            demanded,
            module.link_facts.direct_eval,
            &module.link_facts.removable_declarations,
        )
    });
    let mut pruned = program.clone();
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
    if policy.compact {
        compact_closure_source(
            &mut pruned,
            liveness.as_ref().map(|(reachable, _, _)| reachable),
        );
    }
    let emitted_program = if policy.compact || liveness.is_some() {
        &pruned
    } else {
        program
    };
    let extra_entry_slots: HashSet<u32> = if uses_module_scope(emitted_program) {
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
        policy.generic_only,
        policy.speculate,
        Some(&extra_entry_slots),
    )?;
    let reachable = liveness.as_ref().map(|(reachable, _, _)| reachable);
    if let Some(reachable) = reachable {
        let available = (0..emitted_program.functions.len())
            .map(|index| reachable.contains(&index))
            .collect::<Vec<_>>();
        direct.set_available(&available);
    }
    if emitted_program.has_dynamic_import() {
        output.push_str(
            "__attribute__((weak)) int kiln_graph_dynamic_import(kiln_host *host, const char *importer, kiln_value specifier, kiln_value options, kiln_value *promise, int depth, kiln_value *thrown) {\n    (void)importer; (void)specifier; (void)options;\n    kiln_value err = kiln_c99_undefined();\n    (void)kiln_c99_throw_error(&err, KILN_C99_BUILTIN_TYPE_ERROR, \"no module loader is installed\");\n    return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);\n}\n",
        );
    }
    render_functions(
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
    );
    render_entry(
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
    )?;
    if commonjs {
        /* The launcher cannot manufacture native objects without coupling itself to this private
         * value layout. Let the translation unit that owns the layout construct the wrapper
         * values instead. The helper is linked only into native graphs; it is not part of the
         * public scalar embedding header or ABI. */
        writeln!(
            output,
            "int {symbol}_commonjs_wrapper(kiln_value *imports, size_t import_count, const char *filename, size_t filename_len, const char *dirname, size_t dirname_len, kiln_commonjs_require_fn require_call, void *require_context) {{\n    kiln_value initial;\n    kiln_value module;\n    int status;\n    if (imports == NULL || import_count != (size_t)5) return KILN_C99_INVALID_ARGUMENT;\n    status = kiln_c99_new_object(&initial);\n    if (status == KILN_C99_OK) status = kiln_c99_new_object(&module);\n    if (status == KILN_C99_OK) status = kiln_c99_put(NULL, module, \"exports\", 7, initial, 0, NULL);\n    if (status == KILN_C99_OK) status = kiln_c99_commonjs_require_value(&imports[1], require_call, require_context);\n    if (status != KILN_C99_OK) return status;\n    imports[0] = initial;\n    imports[2] = module;\n    imports[3] = kiln_c99_copy_text(filename, filename_len);\n    imports[4] = kiln_c99_copy_text(dirname, dirname_len);\n    return KILN_C99_OK;\n}}"
        )
        .unwrap();
    }
    render_namespace_wrapper(&mut output, symbol);
    if callables {
        render_module_realm_wrapper(&mut output, symbol, unit_symbol);
    }
    Ok(output)
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
        "void {symbol}_publish_exports(kiln_value *exports) {{\n    (void)exports;\n}}"
    )
    .unwrap();
    render_namespace_wrapper(&mut output, symbol);
    render_module_realm_wrapper(&mut output, symbol, symbol);
    output
}

pub(crate) fn instruction_diagnostic(instruction: &Instruction, constants: &[Constant]) -> String {
    let debug = format!("{instruction:?}");
    let mut operation = debug
        .split([' ', '(', '{'])
        .next()
        .unwrap_or("Instruction")
        .to_owned();
    let named = match instruction {
        Instruction::CallMethod { name, .. }
        | Instruction::GetProperty(name)
        | Instruction::SetProperty(name) => constants.get(*name as usize),
        Instruction::SetPropertyStrict { name, .. } => constants.get(*name as usize),
        _ => None,
    };
    if let Some(Constant::String(name)) = named {
        operation.push('(');
        operation.push_str(&name.to_scalar_string().unwrap_or_default());
        operation.push(')');
    }
    // `@` survives the filter because a reserved key is spelled `@@iterator` everywhere else in
    // this backend, and stripping the prefix here made the diagnostic name a property the program
    // does not have: `CallMethod(iterator)` reads as a call to something called `iterator`, when
    // what refused was `for...of` reaching for `Symbol.iterator`. That cost real time on a bundled
    // `marked` -- the operation named one thing and the lookup did another.
    operation
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric() || b"_.$:-()@".contains(byte))
        .map(char::from)
        .collect()
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
    }
}

/// Whether this proto's own code reads the per-frame globals directly, ignoring anything it
/// calls or creates.
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
fn proto_reads_frame_directly(proto: &FunctionProto) -> bool {
    proto.generator
        || proto.asynchronous
        || proto.async_generator
        || proto.code.iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::LoadArguments | Instruction::LoadNewTarget
            )
        })
}

/// Whether a call into each of `program.functions` must publish the per-frame context, computed
/// once for the whole program and threaded to every user -- `direct_callable`, the fast-clone and
/// direct-call eligibility checks in `emit_graph.rs`, and the `kiln_c99_closure_of(built)
/// ->publishes_frame` write at `MakeFunction` -- rather than recomputed per call site. The result
/// rides alongside `CallPlan::typed_entries` and friends, cloned into every function's plan.
///
/// Proto `i` publishes when [`proto_reads_frame_directly`] says so, or when its code contains
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
    let mut publishes: Vec<bool> = program
        .functions
        .iter()
        .map(proto_reads_frame_directly)
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

fn host_op_code(operation: HostOp) -> u16 {
    operation as u16
}

fn hash_program_pair_with_policy(program: &Program, policy: &EmissionPolicy) -> (u64, u64) {
    (
        hash_program_with_policy_and_seed(program, policy, 0xcbf29ce484222325),
        hash_program_with_policy_and_seed(program, policy, 0x84222325cbf29ce4),
    )
}

fn hash_program_with_policy_and_seed(program: &Program, policy: &EmissionPolicy, seed: u64) -> u64 {
    let mut hash = hash_program_with_seed(program, seed);
    let mut canonical_policy;
    let policy = if policy.speculate == Some(true) {
        canonical_policy = policy.clone();
        canonical_policy.speculate = None;
        &canonical_policy
    } else {
        policy
    };
    if *policy == EmissionPolicy::default() {
        return hash;
    }
    let (entropy_tag, entropy_seed) = match policy.entropy {
        EmissionEntropyPolicy::Deterministic => (0, None),
        EmissionEntropyPolicy::Seeded(seed) => (1, Some(seed)),
        EmissionEntropyPolicy::Host => (2, None),
    };
    for value in [
        0x70,
        u8::from(policy.host_clock),
        entropy_tag,
        u8::from(policy.decoupled_prelude),
        u8::from(policy.compact),
        u8::from(policy.generic_only),
        u8::from(!matches!(policy.speculate, Some(false))),
    ] {
        hash ^= u64::from(value);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if let Some(seed) = entropy_seed {
        for value in seed.to_le_bytes() {
            hash ^= u64::from(value);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash_string_policy(&mut hash, &policy.timezone);
    hash_string_policy(&mut hash, &policy.locale);
    if let Some(slots) = &policy.live_module_slots {
        hash ^= 0x74;
        hash = hash.wrapping_mul(0x100000001b3);
        for slot in slots.iter() {
            for value in slot.to_le_bytes() {
                hash ^= u64::from(value);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    hash
}

fn hash_string_policy(hash: &mut u64, policy: &EmissionStringPolicy) {
    let (tag, named) = match policy {
        EmissionStringPolicy::Deterministic => (0_u8, None),
        EmissionStringPolicy::Named(value) => (1, Some(value.as_str())),
        EmissionStringPolicy::Host => (2, None),
    };
    *hash ^= u64::from(tag);
    *hash = hash.wrapping_mul(0x100000001b3);
    if let Some(value) = named {
        for byte in value.as_bytes() {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
        *hash ^= 0xff;
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

#[cfg(test)]
fn hash_program(program: &Program) -> u64 {
    hash_program_with_seed(program, 0xcbf29ce484222325)
}

fn hash_program_with_seed(program: &Program, seed: u64) -> u64 {
    let mut hash = seed;
    fn byte(hash: &mut u64, value: u8) {
        *hash ^= u64::from(value);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    fn word(hash: &mut u64, value: u64) {
        for value in value.to_le_bytes() {
            byte(hash, value);
        }
    }
    word(&mut hash, u64::from(C_ABI_VERSION));
    word(&mut hash, u64::from(HOST_ABI_VERSION));
    word(&mut hash, u64::from(program.version));
    word(&mut hash, program.constants.len() as u64);
    for constant in &program.constants {
        match constant {
            Constant::Undefined => byte(&mut hash, 0),
            Constant::Null => byte(&mut hash, 1),
            Constant::Boolean(value) => {
                byte(&mut hash, 2);
                byte(&mut hash, u8::from(*value));
            }
            Constant::Number(bits) => {
                byte(&mut hash, 3);
                word(&mut hash, *bits);
            }
            Constant::String(value) => {
                byte(&mut hash, 4);
                word(&mut hash, value.len() as u64);
                for value in value.units() {
                    word(&mut hash, u64::from(value));
                }
            }
            // Tag 5, appended: a persisted key never renumbers.
            Constant::BigInt(digits) => {
                byte(&mut hash, 5);
                word(&mut hash, digits.len() as u64);
                for digit in digits.bytes() {
                    byte(&mut hash, digit);
                }
            }
        }
    }
    word(&mut hash, program.code.len() as u64);
    for instruction in &program.code {
        match instruction {
            // Tags are a persisted cache key: existing ones never change, new instructions
            // take the next free values. Reusing a tag would make two different programs hash
            // alike and serve one's object file for the other.
            Instruction::CallMethod { name, argc } => {
                byte(&mut hash, 31);
                word(&mut hash, u64::from(*name));
                word(&mut hash, u64::from(*argc));
            }
            Instruction::CallComputed { argc } => {
                byte(&mut hash, 32);
                word(&mut hash, u64::from(*argc));
            }
            Instruction::SuperForward => byte(&mut hash, 36),
            Instruction::SetPrototype => byte(&mut hash, 34),
            Instruction::LoadPrototype => byte(&mut hash, 71),
            Instruction::LoadSuperProperty(value) => {
                byte(&mut hash, 72);
                word(&mut hash, u64::from(*value));
            }
            Instruction::LoadSuperElement => byte(&mut hash, 73),
            Instruction::SetSuperPropertyStrict { name, strict } => {
                byte(&mut hash, 80);
                word(&mut hash, u64::from(*name));
                byte(&mut hash, u8::from(*strict));
            }
            Instruction::SetSuperElementStrict { strict } => {
                byte(&mut hash, 81);
                byte(&mut hash, u8::from(*strict));
            }
            Instruction::SuperCall { argc } => {
                byte(&mut hash, 35);
                word(&mut hash, u64::from(*argc));
            }
            Instruction::PushHandler(target) => {
                byte(&mut hash, 38);
                word(&mut hash, u64::from(*target));
            }
            Instruction::PopHandler => byte(&mut hash, 39),
            Instruction::Yield => byte(&mut hash, 42),
            Instruction::YieldStar(target) => {
                byte(&mut hash, 197);
                word(&mut hash, u64::from(*target));
            }
            Instruction::Await => byte(&mut hash, 60),
            Instruction::Throw => byte(&mut hash, 40),
            Instruction::LoadArguments => byte(&mut hash, 37),
            Instruction::RestArguments(count) => {
                byte(&mut hash, 58);
                word(&mut hash, u64::from(*count));
            }
            Instruction::LoadThis => byte(&mut hash, 33),
            Instruction::LoadNewTarget => byte(&mut hash, 59),
            Instruction::Dup2 => byte(&mut hash, 27),
            Instruction::IterableToArray => byte(&mut hash, 44),
            Instruction::SpreadInto => byte(&mut hash, 41),
            Instruction::SpreadObject => byte(&mut hash, 45),
            Instruction::DeleteComputed => byte(&mut hash, 47),
            Instruction::DeleteComputedStrict { strict } => {
                byte(&mut hash, 66);
                byte(&mut hash, u8::from(*strict));
            }
            Instruction::Apply => byte(&mut hash, 48),
            Instruction::SuperApply => byte(&mut hash, 61),
            // 75: the next free tag. Never reuse one -- two programs hashing alike would serve
            // one's object file for the other.
            Instruction::DirectEval(_) => byte(&mut hash, 62),
            Instruction::DynamicImport => byte(&mut hash, 63),
            Instruction::DynamicImportWithOptions => byte(&mut hash, 67),
            Instruction::ToObject => byte(&mut hash, 64),
            Instruction::WithHas(name) => {
                byte(&mut hash, 65);
                word(&mut hash, u64::from(*name));
            }
            Instruction::EnumerateKeys => byte(&mut hash, 50),
            Instruction::PushHole => byte(&mut hash, 55),
            Instruction::ThrowReference(value) => {
                byte(&mut hash, 57);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreGlobalBinding(value) => {
                byte(&mut hash, 74);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreGlobalBindingStrict(value) => {
                byte(&mut hash, 77);
                word(&mut hash, u64::from(*value));
            }
            Instruction::RequireGlobalBinding(value) => {
                byte(&mut hash, 78);
                word(&mut hash, u64::from(*value));
            }
            Instruction::LoadGlobalBindingOrUndefined(value) => {
                byte(&mut hash, 79);
                word(&mut hash, u64::from(*value));
            }
            Instruction::DeleteGlobalBinding(value) => {
                byte(&mut hash, 75);
                word(&mut hash, u64::from(*value));
            }
            Instruction::DeclareGlobalBinding { name, configurable } => {
                byte(&mut hash, 76);
                word(&mut hash, u64::from(*name));
                byte(&mut hash, u8::from(*configurable));
            }
            Instruction::NewRegExp { pattern, flags } => {
                byte(&mut hash, 56);
                word(&mut hash, u64::from(*pattern));
                word(&mut hash, u64::from(*flags));
            }
            Instruction::GetPrivate(value) => {
                byte(&mut hash, 53);
                word(&mut hash, u64::from(*value));
            }
            Instruction::SetPrivate(value) => {
                byte(&mut hash, 54);
                word(&mut hash, u64::from(*value));
            }
            Instruction::DefineAccessor { setter, enumerable } => {
                byte(&mut hash, 52);
                byte(&mut hash, u8::from(*setter));
                byte(&mut hash, u8::from(*enumerable));
            }
            Instruction::DefineMethod => byte(&mut hash, 43),
            Instruction::DefineNamedProperty => byte(&mut hash, 194),
            Instruction::LoadTemplateObject(site) => {
                byte(&mut hash, 195);
                word(&mut hash, u64::from(*site));
            }
            Instruction::DefinePrivate { name, kind } => {
                byte(&mut hash, 70);
                word(&mut hash, u64::from(*name));
                byte(
                    &mut hash,
                    match kind {
                        kiln_ir::PrivateKind::Method => 0,
                        kiln_ir::PrivateKind::Getter => 1,
                        kiln_ir::PrivateKind::Setter => 2,
                    },
                );
            }
            Instruction::MarkUninitialized(value) => {
                byte(&mut hash, 51);
                word(&mut hash, u64::from(*value));
            }
            Instruction::ConstructSpread => byte(&mut hash, 49),
            Instruction::DeleteProperty(value) => {
                byte(&mut hash, 46);
                word(&mut hash, u64::from(*value));
            }
            Instruction::DeletePropertyStrict { name, strict } => {
                byte(&mut hash, 67);
                word(&mut hash, u64::from(*name));
                byte(&mut hash, u8::from(*strict));
            }
            Instruction::NewArray { count } => {
                byte(&mut hash, 28);
                word(&mut hash, u64::from(*count));
            }
            Instruction::GetComputed => byte(&mut hash, 29),
            Instruction::ToPropertyKey => byte(&mut hash, 193),
            Instruction::SetComputed => byte(&mut hash, 30),
            Instruction::SetComputedStrict { strict } => {
                byte(&mut hash, 68);
                byte(&mut hash, u8::from(*strict));
            }
            Instruction::LoadBuiltin(builtin) => {
                byte(&mut hash, 25);
                word(&mut hash, *builtin as u64);
            }
            Instruction::New { argc } => {
                byte(&mut hash, 26);
                word(&mut hash, u64::from(*argc));
            }
            Instruction::NewObject { count } => {
                byte(&mut hash, 22);
                word(&mut hash, u64::from(*count));
            }
            Instruction::GetProperty(value) => {
                byte(&mut hash, 23);
                word(&mut hash, u64::from(*value));
            }
            Instruction::SetProperty(value) => {
                byte(&mut hash, 24);
                word(&mut hash, u64::from(*value));
            }
            Instruction::SetPropertyStrict { name, strict } => {
                byte(&mut hash, 69);
                word(&mut hash, u64::from(*name));
                byte(&mut hash, u8::from(*strict));
            }
            Instruction::NewCell(value) => {
                byte(&mut hash, 17);
                word(&mut hash, u64::from(*value));
            }
            Instruction::LoadCell(value) => {
                byte(&mut hash, 18);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreCell(value) => {
                byte(&mut hash, 19);
                word(&mut hash, u64::from(*value));
            }
            Instruction::LoadUpvalue(value) => {
                byte(&mut hash, 20);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreUpvalue(value) => {
                byte(&mut hash, 21);
                word(&mut hash, u64::from(*value));
            }
            Instruction::MakeFunction(value) => {
                byte(&mut hash, 13);
                word(&mut hash, u64::from(*value));
            }
            Instruction::Call { argc } => {
                byte(&mut hash, 14);
                word(&mut hash, u64::from(*argc));
            }
            Instruction::LoadGlobal(value) => {
                byte(&mut hash, 15);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreGlobal(value) => {
                byte(&mut hash, 16);
                word(&mut hash, u64::from(*value));
            }
            Instruction::Const(value) => {
                byte(&mut hash, 0);
                word(&mut hash, u64::from(*value));
            }
            Instruction::LoadLocal(value) => {
                byte(&mut hash, 1);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreLocal(value) => {
                byte(&mut hash, 2);
                word(&mut hash, u64::from(*value));
            }
            Instruction::Pop => byte(&mut hash, 3),
            Instruction::Dup => byte(&mut hash, 4),
            Instruction::Unary(value) => {
                byte(&mut hash, 5);
                word(&mut hash, u64::from(unary_code(*value)));
            }
            Instruction::Binary(value) => {
                byte(&mut hash, 6);
                word(&mut hash, u64::from(binary_code(*value)));
            }
            Instruction::Jump(value) => {
                byte(&mut hash, 7);
                word(&mut hash, u64::from(*value));
            }
            Instruction::JumpIfFalse(value) => {
                byte(&mut hash, 8);
                word(&mut hash, u64::from(*value));
            }
            Instruction::JumpIfTruthy(value) => {
                byte(&mut hash, 9);
                word(&mut hash, u64::from(*value));
            }
            Instruction::JumpIfNotNullish(value) => {
                byte(&mut hash, 10);
                word(&mut hash, u64::from(*value));
            }
            Instruction::JumpIfReturnSignal(value) => {
                byte(&mut hash, 65);
                word(&mut hash, u64::from(*value));
            }
            Instruction::HostCall { op, argc } => {
                byte(&mut hash, 11);
                word(&mut hash, u64::from(host_op_code(*op)));
                word(&mut hash, u64::from(*argc));
            }
            Instruction::Return => byte(&mut hash, 12),
            Instruction::Nop => byte(&mut hash, 196),
        }
    }
    word(&mut hash, u64::from(program.local_count));
    // Functions are a flat, stable table.  Keep the table marker and count even when it is
    // empty: otherwise an empty table and a stream ending at local_count could be confused by
    // future extensions.  MakeFunction operands are table indices, so hashing the table in
    // order also handles cyclic references without recursive traversal.
    byte(&mut hash, 0x60);
    word(&mut hash, program.functions.len() as u64);
    for function in &program.functions {
        hash_function_proto(&mut hash, function);
    }
    // Appended last, after every byte that was already a persisted cache key. The source is part
    // of the program's identity now that `Function.prototype.toString` answers with it: two units
    // with identical bytecode and different text are not interchangeable, and a cached artifact
    // keyed without this would answer with whichever text was compiled first.
    byte(&mut hash, 0x63);
    word(&mut hash, program.source.len() as u64);
    for value in program.source.bytes() {
        byte(&mut hash, value);
    }
    if !program.template_sites.is_empty() {
        byte(&mut hash, 0x64);
        word(&mut hash, program.template_sites.len() as u64);
        for site in &program.template_sites {
            word(&mut hash, site.cooked.len() as u64);
            for segment in &site.cooked {
                match segment {
                    Some(text) => {
                        byte(&mut hash, 1);
                        word(&mut hash, text.len() as u64);
                        for unit in text.units() {
                            word(&mut hash, u64::from(unit));
                        }
                    }
                    None => byte(&mut hash, 0),
                }
            }
            word(&mut hash, site.raw.len() as u64);
            for segment in &site.raw {
                word(&mut hash, segment.len() as u64);
                for unit in segment.units() {
                    word(&mut hash, u64::from(unit));
                }
            }
        }
    }
    hash
}

fn hash_function_proto(hash: &mut u64, function: &FunctionProto) {
    fn byte(hash: &mut u64, value: u8) {
        *hash ^= u64::from(value);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    fn word(hash: &mut u64, value: u64) {
        for value in value.to_le_bytes() {
            byte(hash, value);
        }
    }

    byte(hash, 0x61);
    byte(hash, u8::from(function.asynchronous));
    // Identity, not display: a present written name shadows any inferred one
    // (equal programs must hit), while an empty written name is distinct
    // from no written name (different statics emitted). `effective_name_units`
    // filters empties for display and would conflate the last two, serving
    // one's object file for the other.
    match function
        .name
        .as_deref()
        .map(kiln_ir::StringUnits::from)
        .or_else(|| function.inferred_name.clone())
    {
        Some(name) => {
            byte(hash, 1);
            word(hash, name.unit_len() as u64);
            for unit in name.units() {
                word(hash, u64::from(unit));
            }
        }
        None => byte(hash, 0),
    }
    byte(hash, u8::from(function.arrow));
    byte(hash, u8::from(function.generator));
    // Appended after `generator`, which is where a new flag has to go: the bytes before it are a
    // persisted cache key and cannot move.
    byte(hash, u8::from(function.async_generator));
    byte(hash, u8::from(function.is_method));
    word(hash, u64::from(function.parameter_count));
    word(hash, u64::from(function.local_count));

    // Reuse the canonical instruction encoder, including its persisted tag assignments. Seed it
    // with the hash accumulated so far and retain the resulting state directly: reducing the
    // function body to an independently seeded 64-bit digest here would introduce an avoidable
    // collision boundary inside the program identity. The synthetic program's empty function
    // table is also what makes numeric MakeFunction cycles terminate without recursive hashing.
    // Which bytes of the source this body is, appended after the flags that were already
    // persisted so the prefix does not move.
    byte(hash, 0x64);
    word(hash, u64::from(function.span.start));
    word(hash, u64::from(function.span.end));

    byte(hash, 0x62);
    *hash = hash_program_with_seed(
        &Program {
            version: BYTECODE_VERSION,
            constants: Vec::new(),
            code: function.code.clone(),
            local_count: 0,
            functions: Vec::new(),
            template_sites: Vec::new(),
            // The body's own bytes are hashed by the caller; this synthetic program exists only
            // to reach the canonical instruction encoder.
            source: Arc::from(""),
        },
        *hash,
    );

    byte(hash, 0x63);
    word(hash, function.upvalues.len() as u64);
    for upvalue in &function.upvalues {
        match upvalue {
            Upvalue::ParentLocal(index) | Upvalue::NamedParentLocal { slot: index, .. } => {
                byte(hash, 0);
                word(hash, u64::from(*index));
            }
            Upvalue::ParentUpvalue(index) | Upvalue::NamedParentUpvalue { index, .. } => {
                byte(hash, 1);
                word(hash, u64::from(*index));
            }
        }
    }
}

fn hash_module_pair_with_policy(module: &ModuleProgram, policy: &EmissionPolicy) -> (u64, u64) {
    (
        hash_module_with_policy_and_seed(module, policy, 0xcbf29ce484222325),
        hash_module_with_policy_and_seed(module, policy, 0x84222325cbf29ce4),
    )
}

fn hash_module_with_policy_and_seed(
    module: &ModuleProgram,
    policy: &EmissionPolicy,
    seed: u64,
) -> u64 {
    let mut hash = hash_module_with_seed(module, seed);
    if *policy == EmissionPolicy::default() {
        return hash;
    }
    let (entropy_tag, entropy_seed) = match policy.entropy {
        EmissionEntropyPolicy::Deterministic => (0, None),
        EmissionEntropyPolicy::Seeded(seed) => (1, Some(seed)),
        EmissionEntropyPolicy::Host => (2, None),
    };
    for value in [
        0x71,
        u8::from(policy.host_clock),
        entropy_tag,
        u8::from(policy.decoupled_prelude),
        u8::from(policy.compact),
        u8::from(policy.generic_only),
    ] {
        hash ^= u64::from(value);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if let Some(seed) = entropy_seed {
        for value in seed.to_le_bytes() {
            hash ^= u64::from(value);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash_string_policy(&mut hash, &policy.timezone);
    hash_string_policy(&mut hash, &policy.locale);
    if policy.type_profile_record {
        hash ^= 0x72;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if let Some(prof) = &policy.type_profile {
        hash ^= 0x73;
        hash = hash.wrapping_mul(0x100000001b3);
        if let Ok(json) = prof.to_json() {
            for b in json.as_bytes() {
                hash ^= u64::from(*b);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    if let Some(slots) = &policy.live_module_slots {
        hash ^= 0x74;
        hash = hash.wrapping_mul(0x100000001b3);
        for slot in slots.iter() {
            for value in slot.to_le_bytes() {
                hash ^= u64::from(value);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    hash
}

fn hash_logical_path_pair(logical_path: &str) -> (u64, u64) {
    (
        hash_logical_path_with_seed(logical_path, 0xcbf29ce484222325),
        hash_logical_path_with_seed(logical_path, 0x84222325cbf29ce4),
    )
}

fn hash_logical_path_with_seed(logical_path: &str, seed: u64) -> u64 {
    let mut hash = seed;
    for byte in b"kiln-logical-module-path\0" {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for byte in (logical_path.len() as u64).to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for byte in logical_path.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn hash_module_with_seed(module: &ModuleProgram, seed: u64) -> u64 {
    let mut hash = hash_program_with_seed(&module.program, seed);
    fn byte(hash: &mut u64, value: u8) {
        *hash ^= u64::from(value);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    fn word(hash: &mut u64, value: u64) {
        for value in value.to_le_bytes() {
            byte(hash, value);
        }
    }
    fn text(hash: &mut u64, value: &str) {
        word(hash, value.len() as u64);
        for byte in value.bytes() {
            *hash ^= u64::from(byte);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
    }

    // Tags and lengths make the linkage stream unambiguous even when names happen to share
    // prefixes.  Preserve the source order promised by ModuleProgram for all three lists.
    byte(&mut hash, 0x40);
    word(&mut hash, module.dependencies.len() as u64);
    for dependency in &module.dependencies {
        text(&mut hash, dependency);
    }
    byte(&mut hash, 0x41);
    word(&mut hash, module.imports.len() as u64);
    for binding in &module.imports {
        text(&mut hash, &binding.source);
        text(&mut hash, &binding.imported);
        text(&mut hash, &binding.local);
        word(&mut hash, u64::from(binding.local_slot));
    }
    byte(&mut hash, 0x42);
    word(&mut hash, module.exports.len() as u64);
    for binding in &module.exports {
        text(&mut hash, &binding.exported);
        word(&mut hash, u64::from(binding.local_slot));
    }
    // Appended last, after every byte that was already a persisted cache key -- the same rule
    // the source text follows in `hash_program_with_seed`. Strictness changes the emitted C: a
    // sloppy body's closure carries the bit that gives a bare call the global object as `this`.
    // Two modules with identical bytecode and different strictness are not interchangeable, and
    // a key without this would serve one's object file for the other. The padding matches the
    // emitter's: a table shorter than the function list reads strict.
    byte(&mut hash, 0x43);
    word(&mut hash, module.program.functions.len() as u64);
    for index in 0..module.program.functions.len() {
        byte(
            &mut hash,
            u8::from(
                module
                    .strictness
                    .functions
                    .get(index)
                    .copied()
                    .unwrap_or(true),
            ),
        );
    }
    hash
}

#[cfg(test)]
mod tests;

/// The reserved key an instance stores its class's brand under, derived from a private name.
///
/// `@@private:{scope}:{member}` becomes `@@private:{scope}:%brand`: one brand per class, shared
/// by every private member it declares. Derived here rather than passed, so the instruction
/// keeps the shape the interpreter gives it.
fn private_brand_key_of(name: &str) -> String {
    match name
        .strip_prefix("@@private:")
        .and_then(|rest| rest.split_once(':'))
    {
        Some((scope, _)) => format!("@@private:{scope}:%brand"),
        None => name.to_owned(),
    }
}
