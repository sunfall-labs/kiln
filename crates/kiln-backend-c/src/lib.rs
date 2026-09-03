//! Deterministic C99 emission for Kiln's scalar bytecode.
//!
//! This crate is deliberately a backend, not a C runtime.  The generated translation unit owns
//! the value representation and calls a host callback for every [`kiln_ir::Instruction::HostCall`]
//! using the numeric [`kiln_host_abi::HostOp`] value.  In particular, it never writes stdout (or
//! otherwise bakes CLI policy into an object file).

use std::{collections::HashSet, fmt::Write as _, sync::Arc};

use kiln_host_abi::{HOST_ABI_VERSION, HostOp};
use kiln_ir::{
    BYTECODE_VERSION, BinaryOp, Constant, FunctionProto, Instruction, ModuleProgram, Program,
    UnaryOp, Upvalue,
};
use kiln_native_number::{NUMBER_FORMAT_VERSION, NUMBER_PARSE_VERSION};

/// Version of the generated C ABI.  This is independent from the bytecode and HostOps versions.
pub const C_ABI_VERSION: u32 = 1;

/// Revision of the complete generated translation-unit format.
///
/// This is deliberately independent from [`C_ABI_VERSION`]: diagnostics and private helper code
/// can change emitted object bytes without changing the public embedding layout. Native artifact
/// keys include this revision so those objects are never reused across generator changes.
pub const C_GENERATOR_VERSION: u32 = 18;

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
        concat!("int ", stringify!($name), "(const uint8_t *key, uint64_t key_len, uint64_t *out_id);\n")
    };
    ($group:ident, $name:ident, realm_symbol_key_for) => {
        concat!("int ", stringify!($name), "(uint64_t id, const uint8_t **out_key, uint64_t *out_key_len);\n")
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
    let source = render(program, program_hash, &symbol, &policy);
    Ok(CModule {
        symbol,
        source,
        program_hash,
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
    let source = render_module(module, program_hash, &symbol, &policy, false, &symbol);
    Ok(CModule {
        symbol,
        source,
        program_hash,
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
    validate_logical_path(logical_path)?;
    validate_module(module)?;
    let (program_hash, _) = hash_module_pair_with_policy(module, &policy);
    let (path_hash, path_hash_seeded) = hash_logical_path_pair(logical_path);
    let symbol = format!("kiln_module_path_{path_hash:016x}{path_hash_seeded:016x}");
    let source = render_module(module, program_hash, &symbol, &policy, false, logical_path);
    Ok(CModule {
        symbol,
        source,
        program_hash,
    })
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
    let source = render_module(module, program_hash, &symbol, &policy, true, logical_path);
    Ok(CModule {
        symbol,
        source,
        program_hash,
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
    validate_logical_path(logical_path)?;
    validate_module(module)?;
    if body.as_bytes().contains(&0) {
        return Err(EmitError::InvalidProgram {
            message: "runtime module body contains an embedded NUL".to_owned(),
        });
    }
    let (program_hash, _) = hash_module_pair(module);
    let (path_hash, path_hash_seeded) = hash_logical_path_pair(logical_path);
    let symbol = format!("kiln_module_path_{path_hash:016x}{path_hash_seeded:016x}");
    let source = render_runtime_module(module, program_hash, &symbol, body);
    Ok(CModule {
        symbol,
        source,
        program_hash,
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
fn produces_references(program: &Program) -> bool {
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
    reaches(&program.code) || program.functions.iter().any(|proto| reaches(&proto.code))
}

/// Whether a body reads `this`.
///
/// The module entry point has no receiver parameter, so it declares one only when the body asks
/// for it -- and a module's `this` is undefined, because a module is strict and strict mode does
/// not substitute the global object.
fn uses_this(code: &[Instruction]) -> bool {
    code.iter()
        .any(|instruction| matches!(instruction, Instruction::LoadThis))
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
                        if let Upvalue::ParentLocal(slot) = upvalue {
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

/// What a call site does when the callee threw: jump to this body's handler, or hand it on.
/// Where a thrown status goes from the site that produced it.
///
/// A body with a handler jumps to its own unwind. A body without one propagates, except at the
/// entry point, where there is nothing left to propagate to: the throw is uncaught, and the only
/// thing that can still be done with it is say so. That is what `kiln_uncaught` is for, and why
/// the entry is the one place a bare `return status;` would lose the program's own error.
fn unwind_or_return(handlers: &[u32], site: &ReturnSite<'_>) -> &'static str {
    if !handlers.is_empty() {
        "goto kiln_unwind;"
    } else if matches!(site, ReturnSite::Entry { .. }) {
        "goto kiln_uncaught;"
    } else {
        "return status;"
    }
}

/// The entry point's last word on a throw nothing caught.
///
/// The thrown value is converted here rather than handed over, because the embedding signature
/// carries one `kiln_value` and a reference must not cross it. Text is what survives, and text is
/// what a launcher can print.
fn render_uncaught(output: &mut String) {
    if !output.contains("goto kiln_uncaught;") {
        // No site can reach it, and an unused label is a warning in every compiler worth using.
        return;
    }
    output.push_str("kiln_uncaught:;\n");
    output.push_str("    { kiln_value message = kiln_c99_text(*thrown); kiln_value site = kiln_c99_control_diagnostic(); if (message.tag == KILN_VALUE_STRING && site.tag == KILN_VALUE_STRING && site.len != 0) { kiln_value separator = kiln_c99_copy_text(\" at \" , 4); *result = kiln_c99_concat(kiln_c99_concat(message, separator), site); } else { *result = message; } }\n");
    output.push_str("    return KILN_C99_THROWN;\n");
}

fn render_refusal(output: &mut String) {
    output.push_str("kiln_refusal:;\n");
    output.push_str("    *result = kiln_c99_control_diagnostic();\n");
    output.push_str("    return KILN_C99_CONTROL_ERROR;\n");
}

/// The instruction indices a body registers as exception handlers.
///
/// The set, not the count: an unwind has to jump to one of them, and C has no computed goto, so
/// the emitted dispatch is a switch over exactly these labels.
fn handler_targets(code: &[Instruction]) -> Vec<u32> {
    let mut targets: Vec<u32> = code
        .iter()
        .filter_map(|instruction| match instruction {
            Instruction::PushHandler(target) => Some(*target),
            _ => None,
        })
        .collect();
    targets.sort_unstable();
    targets.dedup();
    targets
}

/// Compute which `LoadLocal`, `LoadCell`, and `LoadGlobal` instruction indices can observe an uninitialized binding.
fn compute_guarded_loads(code: &[Instruction]) -> HashSet<usize> {
    let uninit_slots: HashSet<u32> = code
        .iter()
        .filter_map(|instruction| match instruction {
            Instruction::MarkUninitialized(local) => Some(*local),
            _ => None,
        })
        .collect();
    if uninit_slots.is_empty() {
        return HashSet::new();
    }

    let n = code.len();
    let mut in_states: Vec<HashSet<u32>> = vec![HashSet::new(); n];
    let mut worklist: Vec<usize> = (0..n).collect();
    let mut in_worklist: Vec<bool> = vec![true; n];

    // Find handler ranges: PushHandler(target) ... PopHandler
    let mut active_handlers: Vec<usize> = Vec::new();
    let mut inst_handlers: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (pc, inst) in code.iter().enumerate() {
        match inst {
            Instruction::PushHandler(target) => {
                active_handlers.push(*target as usize);
            }
            Instruction::PopHandler => {
                active_handlers.pop();
            }
            _ => {}
        }
        inst_handlers[pc] = active_handlers.clone();
    }

    while let Some(pc) = worklist.pop() {
        in_worklist[pc] = false;
        let in_state = in_states[pc].clone();

        // Compute out_state
        let mut out_state = in_state;
        match code[pc] {
            Instruction::MarkUninitialized(slot) => {
                out_state.insert(slot);
            }
            Instruction::StoreLocal(slot)
            | Instruction::StoreCell(slot)
            | Instruction::StoreGlobal(slot) => {
                out_state.remove(&slot);
            }
            _ => {}
        }

        // Determine successors
        let mut succs = Vec::with_capacity(4);
        match code[pc] {
            Instruction::Jump(target) => {
                succs.push(target as usize);
            }
            Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target) => {
                if pc + 1 < n {
                    succs.push(pc + 1);
                }
                succs.push(target as usize);
            }
            Instruction::Return | Instruction::Throw => {}
            _ => {
                if pc + 1 < n {
                    succs.push(pc + 1);
                }
            }
        }

        // If in an active handler, out_state also reaches handler targets
        if !inst_handlers[pc].is_empty() {
            for &handler_target in &inst_handlers[pc] {
                if handler_target < n {
                    succs.push(handler_target);
                }
            }
        }

        // Propagate to successors
        for succ in succs {
            if succ >= n {
                continue;
            }
            let prev_len = in_states[succ].len();
            in_states[succ].extend(out_state.iter().copied());
            if in_states[succ].len() > prev_len && !in_worklist[succ] {
                worklist.push(succ);
                in_worklist[succ] = true;
            }
        }
    }

    let mut guarded_loads = HashSet::new();
    for (pc, inst) in code.iter().enumerate() {
        match inst {
            Instruction::LoadLocal(slot)
            | Instruction::LoadCell(slot)
            | Instruction::LoadGlobal(slot)
                if in_states[pc].contains(slot) =>
            {
                guarded_loads.insert(pc);
            }
            _ => {}
        }
    }

    guarded_loads
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValType {
    Bottom,
    Number,
    Function(u32),
    Any,
}

impl ValType {
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Bottom, x) | (x, Self::Bottom) => x,
            (Self::Number, Self::Number) => Self::Number,
            (Self::Function(a), Self::Function(b)) if a == b => Self::Function(a),
            _ => Self::Any,
        }
    }
}

fn abstract_interpret(
    code: &[Instruction],
    constants: &[Constant],
    local_count: u32,
    initial_params: &[ValType],
    function_signatures: &[(Vec<ValType>, ValType)],
    current_func: Option<usize>,
    observed_call_args: &mut [Vec<ValType>],
) -> (HashSet<usize>, ValType) {
    let n = code.len();
    if n == 0 {
        return (HashSet::new(), ValType::Bottom);
    }

    let mut in_locals: Vec<Option<Vec<ValType>>> = vec![None; n];
    let mut in_stack: Vec<Option<Vec<ValType>>> = vec![None; n];
    let mut known_numbers: HashSet<usize> = HashSet::new();
    let mut observed_return = ValType::Bottom;

    let mut init_loc = vec![ValType::Any; local_count as usize];
    for (i, &p) in initial_params.iter().enumerate() {
        if i < init_loc.len() && p != ValType::Bottom {
            init_loc[i] = p;
        }
    }
    in_locals[0] = Some(init_loc);
    in_stack[0] = Some(Vec::new());

    let mut worklist = vec![0usize];
    let mut in_worklist = vec![false; n];
    in_worklist[0] = true;

    while let Some(pc) = worklist.pop() {
        in_worklist[pc] = false;
        let Some(cur_locals) = in_locals[pc].clone() else {
            continue;
        };
        let Some(cur_stack) = in_stack[pc].clone() else {
            continue;
        };

        let mut out_locals = cur_locals;
        let mut out_stack = cur_stack;

        match &code[pc] {
            Instruction::Const(idx) => {
                let ty = if matches!(constants.get(*idx as usize), Some(Constant::Number(_))) {
                    ValType::Number
                } else {
                    ValType::Any
                };
                out_stack.push(ty);
            }
            Instruction::MakeFunction(proto_idx) => {
                out_stack.push(ValType::Function(*proto_idx));
            }
            Instruction::LoadLocal(slot) => {
                let ty = out_locals
                    .get(*slot as usize)
                    .copied()
                    .unwrap_or(ValType::Any);
                out_stack.push(ty);
            }
            Instruction::StoreLocal(slot) => {
                let ty = out_stack.pop().unwrap_or(ValType::Any);
                if (*slot as usize) < out_locals.len() {
                    out_locals[*slot as usize] = ty;
                }
            }
            Instruction::Pop => {
                out_stack.pop();
            }
            Instruction::Dup => {
                if let Some(&top) = out_stack.last() {
                    out_stack.push(top);
                }
            }
            Instruction::Dup2 => {
                let len = out_stack.len();
                if len >= 2 {
                    let a = out_stack[len - 2];
                    let b = out_stack[len - 1];
                    out_stack.push(a);
                    out_stack.push(b);
                }
            }
            Instruction::Unary(op) => {
                let val = out_stack.pop().unwrap_or(ValType::Any);
                if val == ValType::Number
                    && matches!(
                        op,
                        UnaryOp::Negate
                            | UnaryOp::Plus
                            | UnaryOp::ToNumeric
                            | UnaryOp::Increment
                            | UnaryOp::Decrement
                    )
                {
                    known_numbers.insert(pc);
                    out_stack.push(ValType::Number);
                } else {
                    known_numbers.remove(&pc);
                    out_stack.push(ValType::Any);
                }
            }
            Instruction::Binary(op) => {
                let right = out_stack.pop().unwrap_or(ValType::Any);
                let left = out_stack.pop().unwrap_or(ValType::Any);
                if left == ValType::Number && right == ValType::Number {
                    if matches!(
                        op,
                        BinaryOp::Add
                            | BinaryOp::Subtract
                            | BinaryOp::Multiply
                            | BinaryOp::Divide
                            | BinaryOp::Remainder
                    ) {
                        known_numbers.insert(pc);
                        out_stack.push(ValType::Number);
                    } else {
                        known_numbers.remove(&pc);
                        out_stack.push(ValType::Any);
                    }
                } else {
                    known_numbers.remove(&pc);
                    out_stack.push(ValType::Any);
                }
            }
            Instruction::Call { argc } => {
                let argc = *argc as usize;
                let callee = if out_stack.len() > argc {
                    out_stack[out_stack.len() - argc - 1]
                } else {
                    ValType::Any
                };
                let target_func = match callee {
                    ValType::Function(idx) => Some(idx as usize),
                    _ => current_func,
                };
                let mut args = Vec::with_capacity(argc);
                for _ in 0..argc {
                    args.push(out_stack.pop().unwrap_or(ValType::Any));
                }
                args.reverse();
                out_stack.pop();

                if let Some(target) = target_func {
                    if target < observed_call_args.len() {
                        for (arg_i, &arg_ty) in args.iter().enumerate() {
                            if arg_i < observed_call_args[target].len() {
                                observed_call_args[target][arg_i] =
                                    observed_call_args[target][arg_i].join(arg_ty);
                            }
                        }
                    }
                    let ret_ty = function_signatures
                        .get(target)
                        .map(|(_, ret)| *ret)
                        .unwrap_or(ValType::Any);
                    out_stack.push(if ret_ty == ValType::Bottom {
                        ValType::Any
                    } else {
                        ret_ty
                    });
                } else {
                    out_stack.push(ValType::Any);
                }
            }
            Instruction::Return => {
                let ret = out_stack.pop().unwrap_or(ValType::Any);
                observed_return = observed_return.join(ret);
            }
            _ => {}
        }

        let mut succs = Vec::with_capacity(2);
        match &code[pc] {
            Instruction::Jump(target) => {
                succs.push(*target as usize);
            }
            Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target) => {
                out_stack.pop();
                if pc + 1 < n {
                    succs.push(pc + 1);
                }
                succs.push(*target as usize);
            }
            Instruction::Return | Instruction::Throw => {}
            _ => {
                if pc + 1 < n {
                    succs.push(pc + 1);
                }
            }
        }

        for succ in succs {
            if succ >= n {
                continue;
            }
            let mut changed = false;
            match (&mut in_locals[succ], &mut in_stack[succ]) {
                (Some(succ_locals), Some(succ_stack)) => {
                    for (i, val) in succ_locals.iter_mut().enumerate() {
                        if let Some(&new_val) = out_locals.get(i) {
                            let merged = val.join(new_val);
                            if *val != merged {
                                *val = merged;
                                changed = true;
                            }
                        }
                    }
                    if succ_stack.len() == out_stack.len() {
                        for (j, val) in succ_stack.iter_mut().enumerate() {
                            let merged = val.join(out_stack[j]);
                            if *val != merged {
                                *val = merged;
                                changed = true;
                            }
                        }
                    }
                }
                (loc, stk) => {
                    *loc = Some(out_locals.clone());
                    *stk = Some(out_stack.clone());
                    changed = true;
                }
            }
            if changed && !in_worklist[succ] {
                worklist.push(succ);
                in_worklist[succ] = true;
            }
        }
    }

    (known_numbers, observed_return)
}

#[allow(clippy::type_complexity)]
fn infer_program_types(
    program: &Program,
) -> (
    Vec<HashSet<usize>>,
    HashSet<usize>,
    Vec<(Vec<ValType>, ValType)>,
) {
    let mut signatures: Vec<(Vec<ValType>, ValType)> = program
        .functions
        .iter()
        .map(|f| {
            (
                vec![ValType::Bottom; f.parameter_count as usize],
                ValType::Bottom,
            )
        })
        .collect();

    for _ in 0..4 {
        let mut observed_call_args: Vec<Vec<ValType>> = program
            .functions
            .iter()
            .map(|f| vec![ValType::Bottom; f.parameter_count as usize])
            .collect();
        let mut observed_returns: Vec<ValType> = vec![ValType::Bottom; program.functions.len()];

        abstract_interpret(
            &program.code,
            &program.constants,
            program.local_count,
            &[],
            &signatures,
            None,
            &mut observed_call_args,
        );

        for (f_idx, proto) in program.functions.iter().enumerate() {
            let (_, ret) = abstract_interpret(
                &proto.code,
                &program.constants,
                proto.local_count,
                &signatures[f_idx].0,
                &signatures,
                Some(f_idx),
                &mut observed_call_args,
            );
            observed_returns[f_idx] = ret;
        }

        let mut changed = false;
        for (f_idx, (params, ret)) in signatures.iter_mut().enumerate() {
            for (p_idx, p_val) in params.iter_mut().enumerate() {
                let obs = observed_call_args[f_idx][p_idx];
                if *p_val != obs {
                    *p_val = obs;
                    changed = true;
                }
            }
            if *ret != observed_returns[f_idx] {
                *ret = observed_returns[f_idx];
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut dummy_call_args: Vec<Vec<ValType>> = program
        .functions
        .iter()
        .map(|f| vec![ValType::Bottom; f.parameter_count as usize])
        .collect();

    let (module_known, _) = abstract_interpret(
        &program.code,
        &program.constants,
        program.local_count,
        &[],
        &signatures,
        None,
        &mut dummy_call_args,
    );

    let mut func_known = Vec::with_capacity(program.functions.len());
    for (f_idx, proto) in program.functions.iter().enumerate() {
        let (k, _) = abstract_interpret(
            &proto.code,
            &program.constants,
            proto.local_count,
            &signatures[f_idx].0,
            &signatures,
            Some(f_idx),
            &mut dummy_call_args,
        );
        func_known.push(k);
    }

    (func_known, module_known, signatures)
}

fn compute_guarded_loads_and_slots(code: &[Instruction]) -> (HashSet<usize>, HashSet<u32>) {
    let guarded = compute_guarded_loads(code);
    let guarded_slots = guarded
        .iter()
        .filter_map(|&idx| match code.get(idx)? {
            Instruction::LoadLocal(slot) | Instruction::LoadCell(slot) => Some(*slot),
            _ => None,
        })
        .collect();
    (guarded, guarded_slots)
}

fn render_fast_inlines(output: &mut String) {
    output.push_str(
        r#"
static inline KILN_ALWAYS_INLINE double kiln_fast_to_double(uint64_t bits) {
    union { uint64_t u; double d; } pun;
    pun.u = bits;
    return pun.d;
}

static inline KILN_ALWAYS_INLINE uint64_t kiln_fast_from_double(double d) {
    union { double d; uint64_t u; } pun;
    pun.d = d;
    return pun.u;
}

static inline KILN_ALWAYS_INLINE kiln_value kiln_fast_number(double val) {
    kiln_value v;
    v.tag = KILN_VALUE_NUMBER;
    v.len = 0;
    v.bits = kiln_fast_from_double(val);
    return v;
}

static inline KILN_ALWAYS_INLINE kiln_value kiln_fast_number_bits(uint64_t bits) {
    kiln_value v;
    v.tag = KILN_VALUE_NUMBER;
    v.len = 0;
    v.bits = bits;
    return v;
}

static inline KILN_ALWAYS_INLINE kiln_value kiln_fast_boolean(int b) {
    kiln_value v;
    v.tag = KILN_VALUE_BOOLEAN;
    v.len = 0;
    v.bits = b ? 1u : 0u;
    return v;
}

static inline KILN_ALWAYS_INLINE kiln_value kiln_fast_undefined(void) {
    kiln_value v;
    v.tag = 0;
    v.len = 0;
    v.bits = 0;
    return v;
}

static inline KILN_ALWAYS_INLINE int kiln_fast_truthy(kiln_value val) {
    if (KILN_LIKELY(val.tag == KILN_VALUE_BOOLEAN)) {
        return val.bits != 0;
    }
    if (KILN_LIKELY(val.tag == KILN_VALUE_NUMBER)) {
        double d = kiln_fast_to_double(val.bits);
        return d != 0.0 && !isnan(d);
    }
    return kiln_c99_truthy(val);
}
"#,
    );
}

fn can_emit_fast_numeric(proto: &FunctionProto, _func_idx: usize, program: &Program) -> bool {
    if proto.parameter_count > 4 {
        return false;
    }
    if proto.generator || binds_at_call(proto) {
        return false;
    }
    if proto.parameter_count == 0
        && proto
            .code
            .iter()
            .any(|i| matches!(i, Instruction::Call { .. }))
    {
        return false;
    }
    let has_compute = proto.code.iter().enumerate().any(|(pos, i)| match i {
        Instruction::Jump(t) => (*t as usize) <= pos,
        Instruction::Call { .. } => true,
        _ => false,
    });
    if !has_compute {
        return false;
    }
    for inst in &proto.code {
        match inst {
            Instruction::LoadLocal(_)
            | Instruction::StoreLocal(_)
            | Instruction::Pop
            | Instruction::Dup
            | Instruction::Return
            | Instruction::Jump(_)
            | Instruction::JumpIfFalse(_)
            | Instruction::JumpIfTruthy(_) => {}
            Instruction::Const(c) => match program.constants.get(*c as usize) {
                Some(Constant::Number(_) | Constant::Undefined | Constant::Null) => {}
                _ => return false,
            },
            Instruction::Binary(
                BinaryOp::Add
                | BinaryOp::Subtract
                | BinaryOp::Multiply
                | BinaryOp::Divide
                | BinaryOp::Remainder
                | BinaryOp::LessThan
                | BinaryOp::LessThanOrEqual
                | BinaryOp::GreaterThan
                | BinaryOp::GreaterThanOrEqual
                | BinaryOp::LooseEqual
                | BinaryOp::LooseNotEqual
                | BinaryOp::StrictEqual
                | BinaryOp::StrictNotEqual
                | BinaryOp::BitwiseAnd
                | BinaryOp::BitwiseOr
                | BinaryOp::BitwiseXor
                | BinaryOp::ShiftLeft
                | BinaryOp::ShiftRight
                | BinaryOp::UnsignedShiftRight,
            ) => {}
            Instruction::Unary(
                UnaryOp::Negate
                | UnaryOp::Plus
                | UnaryOp::ToNumeric
                | UnaryOp::BitwiseNot
                | UnaryOp::Increment
                | UnaryOp::Decrement,
            ) => {}
            Instruction::Call { argc } => {
                if *argc as usize != proto.parameter_count as usize {
                    return false;
                }
            }
            Instruction::LoadUpvalue(_) => {}
            _ => return false,
        }
    }

    if !proto.upvalues.is_empty() {
        // Verify that every LoadUpvalue is strictly consumed as a callee in Call
        let mut sim_stack: Vec<bool> = Vec::new(); // true = Callee, false = Number
        for inst in &proto.code {
            match inst {
                Instruction::LoadLocal(_)
                | Instruction::Pop
                | Instruction::Dup
                | Instruction::Const(_) => {
                    sim_stack.push(false);
                }
                Instruction::StoreLocal(_) => {
                    sim_stack.pop();
                }
                Instruction::Binary(_) => {
                    let right = sim_stack.pop().unwrap_or(false);
                    let left = sim_stack.pop().unwrap_or(false);
                    if left || right {
                        return false;
                    }
                    sim_stack.push(false);
                }
                Instruction::Unary(_) => {
                    let op = sim_stack.pop().unwrap_or(false);
                    if op {
                        return false;
                    }
                    sim_stack.push(false);
                }
                Instruction::Call { argc } => {
                    let count = *argc as usize;
                    for _ in 0..count {
                        if sim_stack.pop().unwrap_or(false) {
                            return false;
                        }
                    }
                    let callee = sim_stack.pop().unwrap_or(false);
                    if !callee {
                        return false;
                    }
                    sim_stack.push(false);
                }
                Instruction::LoadUpvalue(_) => {
                    sim_stack.push(true);
                }
                Instruction::Return => {
                    let ret = sim_stack.pop().unwrap_or(false);
                    if ret {
                        return false;
                    }
                }
                Instruction::JumpIfFalse(_) | Instruction::JumpIfTruthy(_) => {
                    let cond = sim_stack.pop().unwrap_or(false);
                    if cond {
                        return false;
                    }
                }
                _ => {}
            }
        }
    }

    true
}

fn render_fast_numeric_function(
    output: &mut String,
    program: &Program,
    proto: &FunctionProto,
    func_idx: usize,
    symbol: &str,
) {
    let params = if proto.parameter_count == 0 {
        "void".to_string()
    } else {
        (0..proto.parameter_count)
            .map(|i| format!("double arg{i}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    writeln!(
        output,
        "static double {symbol}_fn_{func_idx}_fast({params}) {{"
    )
    .unwrap();
    let heights = validation::compute_stack_heights(&proto.code);
    let max_stack = heights.iter().copied().max().unwrap_or(0) + 4;
    writeln!(output, "    double stack[{max_stack}];").unwrap();
    let locals = (proto.local_count as usize)
        .max(proto.parameter_count as usize)
        .max(1);
    writeln!(output, "    double locals[{locals}];").unwrap();
    for i in 0..proto.parameter_count as usize {
        writeln!(output, "    locals[{i}] = arg{i};").unwrap();
    }
    for i in proto.parameter_count as usize..locals {
        writeln!(output, "    locals[{i}] = 0.0;").unwrap();
    }
    for (pos, inst) in proto.code.iter().enumerate() {
        let sp = heights[pos];
        writeln!(output, "kiln_fast_label_{pos}:;").unwrap();
        match inst {
            Instruction::LoadLocal(slot) => {
                writeln!(output, "    stack[{sp}] = locals[{slot}];").unwrap();
            }
            Instruction::StoreLocal(slot) => {
                let val_idx = sp.saturating_sub(1);
                writeln!(output, "    locals[{slot}] = stack[{val_idx}];").unwrap();
            }
            Instruction::Pop => {}
            Instruction::Dup => {
                let src_idx = sp.saturating_sub(1);
                writeln!(output, "    stack[{sp}] = stack[{src_idx}];").unwrap();
            }
            Instruction::Const(c) => match program.constants.get(*c as usize) {
                Some(Constant::Number(bits)) => {
                    writeln!(
                        output,
                        "    stack[{sp}] = kiln_fast_to_double(UINT64_C({bits}));"
                    )
                    .unwrap();
                }
                Some(Constant::Undefined | Constant::Null) => {
                    writeln!(output, "    stack[{sp}] = 0.0;").unwrap();
                }
                _ => {}
            },
            Instruction::Binary(op) => {
                let a_idx = sp.saturating_sub(2);
                let b_idx = sp.saturating_sub(1);
                match op {
                    BinaryOp::Add => {
                        writeln!(
                            output,
                            "    stack[{a_idx}] = stack[{a_idx}] + stack[{b_idx}];"
                        )
                        .unwrap();
                    }
                    BinaryOp::Subtract => {
                        writeln!(
                            output,
                            "    stack[{a_idx}] = stack[{a_idx}] - stack[{b_idx}];"
                        )
                        .unwrap();
                    }
                    BinaryOp::Multiply => {
                        writeln!(
                            output,
                            "    stack[{a_idx}] = stack[{a_idx}] * stack[{b_idx}];"
                        )
                        .unwrap();
                    }
                    BinaryOp::Divide => {
                        writeln!(
                            output,
                            "    stack[{a_idx}] = stack[{a_idx}] / stack[{b_idx}];"
                        )
                        .unwrap();
                    }
                    BinaryOp::Remainder => {
                        writeln!(
                            output,
                            "    stack[{a_idx}] = fmod(stack[{a_idx}], stack[{b_idx}]);"
                        )
                        .unwrap();
                    }
                    BinaryOp::LessThan => {
                        writeln!(
                            output,
                            "    stack[{a_idx}] = (stack[{a_idx}] < stack[{b_idx}]) ? 1.0 : 0.0;"
                        )
                        .unwrap();
                    }
                    BinaryOp::LessThanOrEqual => {
                        writeln!(
                            output,
                            "    stack[{a_idx}] = (stack[{a_idx}] <= stack[{b_idx}]) ? 1.0 : 0.0;"
                        )
                        .unwrap();
                    }
                    BinaryOp::GreaterThan => {
                        writeln!(
                            output,
                            "    stack[{a_idx}] = (stack[{a_idx}] > stack[{b_idx}]) ? 1.0 : 0.0;"
                        )
                        .unwrap();
                    }
                    BinaryOp::GreaterThanOrEqual => {
                        writeln!(
                            output,
                            "    stack[{a_idx}] = (stack[{a_idx}] >= stack[{b_idx}]) ? 1.0 : 0.0;"
                        )
                        .unwrap();
                    }
                    BinaryOp::LooseEqual | BinaryOp::StrictEqual => {
                        writeln!(
                            output,
                            "    stack[{a_idx}] = (stack[{a_idx}] == stack[{b_idx}]) ? 1.0 : 0.0;"
                        )
                        .unwrap();
                    }
                    BinaryOp::LooseNotEqual | BinaryOp::StrictNotEqual => {
                        writeln!(
                            output,
                            "    stack[{a_idx}] = (stack[{a_idx}] != stack[{b_idx}]) ? 1.0 : 0.0;"
                        )
                        .unwrap();
                    }
                    BinaryOp::BitwiseAnd => {
                        writeln!(output, "    stack[{a_idx}] = (double)((int32_t)stack[{a_idx}] & (int32_t)stack[{b_idx}]);").unwrap();
                    }
                    BinaryOp::BitwiseOr => {
                        writeln!(output, "    stack[{a_idx}] = (double)((int32_t)stack[{a_idx}] | (int32_t)stack[{b_idx}]);").unwrap();
                    }
                    BinaryOp::BitwiseXor => {
                        writeln!(output, "    stack[{a_idx}] = (double)((int32_t)stack[{a_idx}] ^ (int32_t)stack[{b_idx}]);").unwrap();
                    }
                    BinaryOp::ShiftLeft => {
                        writeln!(output, "    stack[{a_idx}] = (double)((int32_t)stack[{a_idx}] << ((uint32_t)stack[{b_idx}] & 31));").unwrap();
                    }
                    BinaryOp::ShiftRight => {
                        writeln!(output, "    stack[{a_idx}] = (double)((int32_t)stack[{a_idx}] >> ((uint32_t)stack[{b_idx}] & 31));").unwrap();
                    }
                    BinaryOp::UnsignedShiftRight => {
                        writeln!(output, "    stack[{a_idx}] = (double)((uint32_t)stack[{a_idx}] >> ((uint32_t)stack[{b_idx}] & 31));").unwrap();
                    }
                    _ => {}
                }
            }
            Instruction::Unary(op) => {
                let val_idx = sp.saturating_sub(1);
                match op {
                    UnaryOp::Negate => {
                        writeln!(output, "    stack[{val_idx}] = -stack[{val_idx}];").unwrap()
                    }
                    UnaryOp::Plus | UnaryOp::ToNumeric => {}
                    UnaryOp::BitwiseNot => {
                        writeln!(
                            output,
                            "    stack[{val_idx}] = (double)(~(int32_t)stack[{val_idx}]);"
                        )
                        .unwrap();
                    }
                    UnaryOp::Increment => writeln!(output, "    stack[{val_idx}] += 1.0;").unwrap(),
                    UnaryOp::Decrement => writeln!(output, "    stack[{val_idx}] -= 1.0;").unwrap(),
                    _ => {}
                }
            }
            Instruction::Jump(target) => {
                writeln!(output, "    goto kiln_fast_label_{target};").unwrap();
            }
            Instruction::JumpIfFalse(target) => {
                let cond_idx = sp.saturating_sub(1);
                writeln!(
                    output,
                    "    if (isnan(stack[{cond_idx}]) || stack[{cond_idx}] == 0.0) goto kiln_fast_label_{target};"
                )
                .unwrap();
            }
            Instruction::JumpIfTruthy(target) => {
                let cond_idx = sp.saturating_sub(1);
                writeln!(
                    output,
                    "    if (!isnan(stack[{cond_idx}]) && stack[{cond_idx}] != 0.0) goto kiln_fast_label_{target};"
                )
                .unwrap();
            }
            Instruction::Return => {
                let ret_idx = sp.saturating_sub(1);
                writeln!(output, "    return stack[{ret_idx}];").unwrap();
            }
            Instruction::LoadUpvalue(_) => {
                writeln!(output, "    stack[{sp}] = 0.0;").unwrap();
            }
            Instruction::Call { argc } => {
                let count = *argc as usize;
                let dest_idx = sp.saturating_sub(count + 1);
                let call_args_str = (0..count)
                    .map(|i| format!("stack[{}]", sp.saturating_sub(count) + i))
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(
                    output,
                    "    stack[{dest_idx}] = {symbol}_fn_{func_idx}_fast({call_args_str});"
                )
                .unwrap();
            }
            _ => {}
        }
    }
    output.push_str("    return 0.0;\n}\n");
}

/// Emit one C function per compiled body, forward-declared so a closure made before a body is
/// rendered can still point at it.
fn render_functions(
    output: &mut String,
    program: &Program,
    plan: &StackPlan,
    symbol: &str,
    diagnostic_site: &str,
    strict_functions: &[bool],
) {
    let (func_known_numbers, _, _signatures) = infer_program_types(program);
    let mut has_fast = vec![false; program.functions.len()];
    for (index, proto) in program.functions.iter().enumerate() {
        if can_emit_fast_numeric(proto, index, program) {
            has_fast[index] = true;
            let params = if proto.parameter_count == 0 {
                "void".to_string()
            } else {
                (0..proto.parameter_count)
                    .map(|i| format!("double arg{i}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            writeln!(output, "static double {symbol}_fn_{index}_fast({params});").unwrap();
        }
    }
    for index in 0..program.functions.len() {
        writeln!(
            output,
            "static int {symbol}_fn_{index}(kiln_host *host, kiln_value *result, kiln_value *upvalues, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown, kiln_generator *frame);"
        )
        .unwrap();
    }
    for (index, proto) in program.functions.iter().enumerate() {
        if has_fast[index] {
            render_fast_numeric_function(output, program, proto, index, symbol);
        }
    }
    for (index, proto) in program.functions.iter().enumerate() {
        if let Some(name) = &proto.name {
            writeln!(
                output,
                "static const char {symbol}_name_{index}[] = {};",
                c_string(name)
            )
            .unwrap();
        }
    }
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
        render_function(
            output,
            program,
            proto,
            &plan.functions,
            index,
            symbol,
            diagnostic_site,
            strict_functions,
            func_known_numbers.get(index).cloned().unwrap_or_default(),
            has_fast[index],
        );
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

/// Declare the handler stack a body needs, or nothing when it catches nothing.
fn render_handler_prologue(output: &mut String, handlers: &[u32], depth: usize, suspends: bool) {
    if handlers.is_empty() {
        return;
    }
    if suspends {
        // A generator's handler stack is in its frame rather than automatic, for exactly the
        // reason its operand stack is: a `try` has to still be catching after the `yield` inside
        // it returns. The macro is how the shared emission below reaches it without every site
        // learning which kind of body it is in.
        output.push_str("    size_t *kiln_handlers = frame->handlers;\n");
        output.push_str("    size_t *kiln_handler_stack = frame->handler_stack;\n");
        output.push_str("#define kiln_handler_top frame->handler_top\n");
        return;
    }
    writeln!(output, "    size_t kiln_handlers[{depth}];").unwrap();
    writeln!(output, "    size_t kiln_handler_stack[{depth}];").unwrap();
    output.push_str("    size_t kiln_handler_top = 0;\n");
}

/// The label a throw jumps to: take the innermost handler, restore the stack as the `try` left
/// it, push what was thrown, and enter the catch.
///
/// A body whose handler stack is empty has nothing to catch with, so the throw keeps travelling
/// -- which is the same path a body with no handlers at all takes.
fn render_unwind(output: &mut String, handlers: &[u32], suspends: bool, uncaught: bool) {
    if handlers.is_empty() {
        return;
    }
    output.push_str("kiln_unwind:;\n");
    output.push_str(if uncaught {
        "    if (kiln_handler_top == 0) goto kiln_uncaught;\n"
    } else {
        "    if (kiln_handler_top == 0) return KILN_C99_THROWN;\n"
    });
    output.push_str("    --kiln_handler_top;\n");
    output.push_str("    kiln_c99_forget_handled_throw();\n");
    output.push_str("    sp = kiln_handler_stack[kiln_handler_top];\n");
    output.push_str("    stack[sp++] = *thrown;\n");
    output.push_str("    switch (kiln_handlers[kiln_handler_top]) {\n");
    for target in handlers {
        writeln!(output, "    case {target}: goto kiln_label_{target};").unwrap();
    }
    output.push_str("    default: return KILN_C99_CONTROL_ERROR;\n    }\n");
    if suspends {
        // The macro stood in for a local that a suspending body does not have; it ends with the
        // body rather than leaking into the next one.
        output.push_str("#undef kiln_handler_top\n");
    }
}

#[allow(clippy::too_many_arguments)]
fn render_function(
    output: &mut String,
    program: &Program,
    proto: &FunctionProto,
    frame_stacks: &[usize],
    index: usize,
    symbol: &str,
    diagnostic_site: &str,
    strict_functions: &[bool],
    known_number_binaries: HashSet<usize>,
    has_fast: bool,
) {
    let max_stack = frame_stacks[index];
    let locals = usize::try_from(proto.local_count).unwrap().max(1);
    // Only when the name is already a C identifier: it goes into a comment, and a name is
    // program text that could otherwise close one.
    if let Some(name) = proto.name.as_ref().filter(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$')
    }) {
        writeln!(output, "/* function {index}: {name} */").unwrap();
    }
    writeln!(
        output,
        "static int {symbol}_fn_{index}(kiln_host *host, kiln_value *result, kiln_value *upvalues, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown, kiln_generator *frame) {{"
    )
    .unwrap();
    output.push_str("    size_t sp = 0;\n    size_t slot;\n");
    // Which of the two `arguments` objects a mention in this body would build. Set on every
    // entry, a generator's resumptions included, because the frame global it writes is shared
    // by whatever else is running and is only restored at the call boundary.
    writeln!(
        output,
        "    kiln_c99_frame_mapped_arguments = {};",
        i32::from(proto.mapped_arguments)
    )
    .unwrap();
    let handlers = handler_targets(&proto.code);
    // A generator and an async function both suspend, and every part of their state has to
    // outlive the call that stopped in the middle of them.
    let suspends = proto.generator || proto.asynchronous;
    let yields: Vec<usize> = proto
        .code
        .iter()
        .enumerate()
        .filter(|(_, instruction)| matches!(instruction, Instruction::Yield | Instruction::Await))
        .map(|(position, _)| position)
        .collect();
    if proto.generator || proto.asynchronous {
        // The frame outlives this call, so neither array can be automatic: both belong to the
        // generator, and every `locals[i]` and `stack[sp]` below reads through them unchanged.
        output.push_str("    kiln_value *stack = frame->stack;\n");
        output.push_str("    kiln_value *locals = frame->locals;\n");
        render_handler_prologue(output, &handlers, proto.code.len(), suspends);
        output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
        output.push_str("    (void)thrown;\n");
        output.push_str("    (void)host; (void)upvalues; (void)depth; (void)receiver;\n");
        output.push_str("    (void)args; (void)argc; (void)slot;\n");
        // Re-entry. A generator that has not started begins at its first instruction; one that
        // has jumps back to just after the `Yield` that suspended it, with the operand stack
        // restored to the height it had there.
        //
        // A body whose parameters were bound at the call re-enters too, even with no `yield` in
        // it at all: the seam is a resume target like any other, and without one `g([{x}])` with
        // an empty body would run its destructuring twice.
        if !yields.is_empty() || binds_at_call(proto) {
            output.push_str("    if (frame->started) {\n        sp = frame->sp;\n");
            // A rejected `await` arrives as a throw at the suspension point rather than as a
            // value, so it goes to the handler stack instead of to the resume label.
            if handlers.is_empty() {
                output.push_str(
                    "        if (frame->raising) { *thrown = frame->sent; return KILN_C99_THROWN; }\n",
                );
            } else {
                output.push_str(
                    "        if (frame->raising) { *thrown = frame->sent; goto kiln_unwind; }\n",
                );
            }
            output.push_str("        switch (frame->resume) {\n");
            for position in &yields {
                writeln!(
                    output,
                    "        case {position}: goto kiln_resume_{position};"
                )
                .unwrap();
            }
            if binds_at_call(proto) {
                // One past the last instruction, so it cannot collide with a `Yield`'s position
                // however the body was compiled -- and unlike a suspension it resumes *at* the
                // instruction rather than after it, because nothing was sent in.
                writeln!(
                    output,
                    "        case {}: goto kiln_label_{};",
                    proto.code.len(),
                    proto.prologue_end
                )
                .unwrap();
            }
            output.push_str("        default: return KILN_C99_CONTROL_ERROR;\n        }\n    }\n");
        }
    } else {
        writeln!(output, "    kiln_value stack[{}];", max_stack.max(1)).unwrap();
        writeln!(output, "    kiln_value locals[{locals}];").unwrap();
        render_handler_prologue(output, &handlers, proto.code.len(), suspends);
        output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
        output.push_str("    (void)thrown; (void)frame;\n");
        output.push_str("    (void)host; (void)upvalues; (void)depth; (void)receiver;\n");
        if has_fast {
            if proto.parameter_count == 0 {
                writeln!(
                    output,
                    "    *result = kiln_fast_number({symbol}_fn_{index}_fast());\n    return KILN_C99_OK;"
                )
                .unwrap();
            } else {
                let param_conds = (0..proto.parameter_count as usize)
                    .map(|i| format!("args[{i}].tag == KILN_VALUE_NUMBER"))
                    .collect::<Vec<_>>()
                    .join(" && ");
                let param_args = (0..proto.parameter_count as usize)
                    .map(|i| format!("kiln_fast_to_double(args[{i}].bits)"))
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(
                    output,
                    "    if (KILN_LIKELY(argc >= (size_t){} && {param_conds})) {{\n        *result = kiln_fast_number({symbol}_fn_{index}_fast({param_args}));\n        return KILN_C99_OK;\n    }}",
                    proto.parameter_count
                )
                .unwrap();
            }
        }
        if proto.parameter_count == 0 {
            output.push_str("    (void)args; (void)argc;\n");
            if locals > 0 {
                writeln!(
                    output,
                    "    for (slot = 0; slot < (size_t){locals}; ++slot) locals[slot] = kiln_fast_undefined();"
                )
                .unwrap();
            }
        } else {
            writeln!(
                output,
                "    for (slot = 0; slot < (size_t){} && slot < argc; ++slot) locals[slot] = args[slot];",
                proto.parameter_count
            )
            .unwrap();
            writeln!(
                output,
                "    for (; slot < (size_t){locals}; ++slot) locals[slot] = kiln_fast_undefined();"
            )
            .unwrap();
        }
    }
    let (guarded_loads, guarded_slots) = compute_guarded_loads_and_slots(&proto.code);
    let emitter = Emitter {
        constants: &program.constants,
        frame_stacks,
        functions: &program.functions,
        strict_functions,
        source: &program.source,
        symbol,
        diagnostic_site,
        callables: produces_references(program),
        handlers,
        site: ReturnSite::Body,
        guarded: guarded_loads,
        guarded_slots,
        known_number_binaries,
        current_func: Some(index),
    };
    for position in 0..proto.code.len() {
        writeln!(output, "kiln_label_{position}:;").unwrap();
        // The seam between FunctionDeclarationInstantiation and the body. Reached twice: once
        // by falling out of the binding the *call* ran, which stops here, and once from the
        // resume switch above, where `binding` is clear and the body carries on.
        if binds_at_call(proto) && position == proto.prologue_end as usize {
            output
                .push_str("    if (frame->binding) { frame->sp = sp; return KILN_C99_YIELDED; }\n");
        }
        emit_instruction(output, &proto.code[position], position, &emitter);
    }
    render_unwind(output, &emitter.handlers, suspends, false);
    // A body whose control reaches the end without a `Return` is a compiler bug, not a program
    // error: the lowering always ends a body with one.
    output.push_str("    goto kiln_refusal;\n");
    render_refusal(output);
    output.push_str("}\n");
}

/// The constant table.
///
/// A string is its bytes. A BigInt is its digits plus a slot for the handle the provider builds
/// from them on first use: parsing is not free and a literal's value never changes, so the unit
/// pays for it once however often the literal is loaded.
fn render_constant_declarations(output: &mut String, program: &Program, symbol: &str) {
    for (index, constant) in program.constants.iter().enumerate() {
        match constant {
            Constant::String(value) => writeln!(
                output,
                "static const char {symbol}_constant_{index}[] = {};",
                c_string(value)
            )
            .unwrap(),
            Constant::BigInt(digits) => {
                writeln!(
                    output,
                    "static const char {symbol}_constant_{index}[] = {};",
                    c_string(digits)
                )
                .unwrap();
                writeln!(output, "static void *{symbol}_bigint_{index} = NULL;").unwrap();
            }
            Constant::Undefined | Constant::Null | Constant::Boolean(_) | Constant::Number(_) => {}
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
        writeln!(output, "static const char *{symbol}_key_{index};").unwrap();
    }
    writeln!(output, "static int {symbol}_keys_ready;").unwrap();
    writeln!(output, "static int {symbol}_intern_keys(void) {{").unwrap();
    for index in keys {
        let Constant::String(text) = &program.constants[*index as usize] else {
            unreachable!("verification rejects a non-string property name before emission")
        };
        writeln!(
            output,
            "    if (({symbol}_key_{index} = kiln_c99_intern_static({symbol}_constant_{index}, {})) == NULL) return KILN_C99_OUT_OF_MEMORY;",
            text.len()
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
fn render_unit_roots(
    output: &mut String,
    symbol: &str,
    module_scope: bool,
    local_count: u32,
    keyed: bool,
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
    if module_scope {
        writeln!(
            output,
            "    if (phase == KILN_GC_PHASE_MARK) kiln_c99_gc_mark_values((kiln_gc_heap *)heap, {symbol}_globals, {});",
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

fn render_module_realm_wrapper(output: &mut String, symbol: &str) {
    writeln!(
        output,
        r#"
typedef struct {symbol}_realm_context {{
    kiln_host *host;
    kiln_value *result;
    const kiln_value *imports;
    size_t import_count;
    kiln_value *exports;
    size_t export_count;
}} {symbol}_realm_context;

static int {symbol}_gc_result_entry(void *result) {{
    return kiln_realm_gc_register_roots(kiln_c99_gc_result_roots, result,
                                        KILN_GC_ROOT_SURVIVES_TEARDOWN);
}}

static int {symbol}_realm_entry(void *opaque) {{
    {symbol}_realm_context *context = ({symbol}_realm_context *)opaque;
    int status;
    if (context == NULL) return KILN_C99_INVALID_ARGUMENT;
    status = {symbol}_body(context->host, context->result, context->imports,
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
    {symbol}_realm_context context;
    uint64_t realm = 0;
    int status;
    int destroy_status;
    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (kiln_realm_is_active()) {{
        (void)kiln_realm_diagnostic_reset();
        status = {symbol}_body(host, result, imports, import_count, exports, export_count);
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
    context.host = host;
    context.result = result;
    context.imports = imports;
    context.import_count = import_count;
    context.exports = exports;
    context.export_count = export_count;
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
            value = *kiln_c99_cell_of(value);
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
        slot->writable = 1;
        slot->enumerable = 1;
        slot->configurable = 0;
        slot->accessor = 0;
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

fn render(program: &Program, program_hash: u64, symbol: &str, policy: &EmissionPolicy) -> String {
    let plan = validate_program(program).expect("render called after validation");
    // A bare `Program` arrives with no goal and no strictness table: it is a scalar unit, and
    // every one this backend is handed came from a Module goal. Strict throughout keeps its
    // emitted bytes exactly what they were.
    let strict_functions = vec![true; program.functions.len()];
    let callables = produces_references(program);
    let entry_symbol = if callables {
        format!("{symbol}_body")
    } else {
        symbol.to_owned()
    };
    let max_stack = plan.body;
    let header_len = if policy.decoupled_prelude {
        PRELUDE_HEADER.len()
    } else {
        HEADER.len()
    };
    let mut output = String::with_capacity(header_len + program.code.len() * 64);
    render_emission_policy(&mut output, policy);
    if policy.decoupled_prelude {
        output.push_str(PRELUDE_HEADER);
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
    render_fast_inlines(&mut output);
    let keys = property_key_constants(program);
    render_key_table(&mut output, program, symbol, &keys);
    if uses_module_scope(program) {
        writeln!(
            output,
            "static kiln_value {symbol}_globals[{}];",
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
    );
    render_functions(
        &mut output,
        program,
        &plan,
        symbol,
        symbol,
        &strict_functions,
    );
    writeln!(
        output,
        "{}int {entry_symbol}(kiln_host *host, kiln_value *result) {{",
        if callables { "static " } else { "" }
    )
    .unwrap();
    output.push_str("    size_t sp = 0;\n");
    writeln!(
        output,
        "    (void)kiln_realm_gc_register_roots({symbol}_gc_roots, NULL, 0);"
    )
    .unwrap();
    render_key_prologue(&mut output, symbol, !keys.is_empty());
    if uses_this(&program.code) {
        output.push_str("    kiln_value receiver = kiln_c99_undefined();\n");
    }
    // The entry point owns the depth counter and the thrown value every body writes into. Both
    // are locals rather than parameters because the embedding signature is fixed and a reference
    // must not cross it: an uncaught throw leaves as text in `*result`.
    if callables {
        output.push_str("    int depth = 0;\n");
        output.push_str("    kiln_value thrown_value = kiln_c99_undefined();\n");
        output.push_str("    kiln_value *thrown = &thrown_value;\n");
    }
    writeln!(output, "    kiln_value stack[{}];", max_stack.max(1)).unwrap();
    if uses_module_scope(program) {
        // Named rather than automatic: an exported closure reads module scope after the entry
        // point has returned, and a frame that has returned is not somewhere to read from.
        writeln!(output, "    kiln_value *locals = {symbol}_globals;").unwrap();
    } else {
        writeln!(
            output,
            "    kiln_value locals[{}];",
            usize::try_from(program.local_count).unwrap().max(1)
        )
        .unwrap();
    }
    render_handler_prologue(
        &mut output,
        &handler_targets(&program.code),
        program.code.len(),
        false,
    );
    output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
    for local in 0..program.local_count {
        writeln!(output, "    locals[{local}] = kiln_c99_undefined();").unwrap();
    }
    let (guarded_loads, guarded_slots) = compute_guarded_loads_and_slots(&program.code);
    let emitter = Emitter {
        constants: &program.constants,
        frame_stacks: &plan.functions,
        functions: &program.functions,
        strict_functions: &strict_functions,
        source: &program.source,
        symbol,
        diagnostic_site: symbol,
        callables,
        handlers: handler_targets(&program.code),
        site: ReturnSite::Entry {
            exports: None,
            commonjs: false,
        },
        guarded: guarded_loads,
        guarded_slots,
        known_number_binaries: HashSet::new(),
        current_func: None,
    };
    for index in 0..program.code.len() {
        writeln!(output, "kiln_label_{index}:;").unwrap();
        emit_instruction(&mut output, &program.code[index], index, &emitter);
    }
    render_unwind(&mut output, &emitter.handlers, false, true);
    render_uncaught(&mut output);
    output.push_str("    goto kiln_refusal;\n");
    render_refusal(&mut output);
    output.push_str("}\n");
    if callables {
        render_scalar_realm_wrapper(&mut output, symbol);
    }
    output
}

fn render_module(
    module: &ModuleProgram,
    program_hash: u64,
    symbol: &str,
    policy: &EmissionPolicy,
    commonjs: bool,
    diagnostic_site: &str,
) -> String {
    let program = &module.program;
    let plan = validate_program(program).expect("render called after validation");
    let callables = produces_references(program);
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
    let entry_symbol = if callables {
        format!("{symbol}_body")
    } else {
        symbol.to_owned()
    };
    let max_stack = plan.body;
    let header_len = if policy.decoupled_prelude {
        PRELUDE_HEADER.len()
    } else {
        HEADER.len()
    };
    let mut output = String::with_capacity(header_len + program.code.len() * 72);
    render_emission_policy(&mut output, policy);
    if policy.decoupled_prelude {
        output.push_str(PRELUDE_HEADER);
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
    render_constant_declarations(&mut output, program, symbol);
    render_fast_inlines(&mut output);
    let keys = property_key_constants(program);
    render_key_table(&mut output, program, symbol, &keys);
    if uses_module_scope(program) {
        writeln!(
            output,
            "static kiln_value {symbol}_globals[{}];",
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
    );
    render_functions(
        &mut output,
        program,
        &plan,
        symbol,
        diagnostic_site,
        &strict_functions,
    );
    writeln!(
        output,
        "{}int {entry_symbol}(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count) {{",
        if callables { "static " } else { "" }
    )
    .unwrap();
    output.push_str("    size_t sp = 0;\n");
    writeln!(
        output,
        "    (void)kiln_realm_gc_register_roots({symbol}_gc_roots, NULL, 0);"
    )
    .unwrap();
    render_key_prologue(&mut output, symbol, !keys.is_empty());
    if uses_this(&program.code) {
        output.push_str("    kiln_value receiver = kiln_c99_undefined();\n");
    }
    // The entry point owns the depth counter and the thrown value every body writes into. Both
    // are locals rather than parameters because the embedding signature is fixed and a reference
    // must not cross it: an uncaught throw leaves as text in `*result`.
    if callables {
        output.push_str("    int depth = 0;\n");
        output.push_str("    kiln_value thrown_value = kiln_c99_undefined();\n");
        output.push_str("    kiln_value *thrown = &thrown_value;\n");
    }
    writeln!(output, "    kiln_value stack[{}];", max_stack.max(1)).unwrap();
    if uses_module_scope(program) {
        // Named rather than automatic: an exported closure reads module scope after the entry
        // point has returned, and a frame that has returned is not somewhere to read from.
        writeln!(output, "    kiln_value *locals = {symbol}_globals;").unwrap();
    } else {
        writeln!(
            output,
            "    kiln_value locals[{}];",
            usize::try_from(program.local_count).unwrap().max(1)
        )
        .unwrap();
    }
    render_handler_prologue(
        &mut output,
        &handler_targets(&program.code),
        program.code.len(),
        false,
    );
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
        if commonjs { 1 } else { module.exports.len() }
    )
    .unwrap();
    for local in 0..program.local_count {
        writeln!(output, "    locals[{local}] = kiln_c99_undefined();").unwrap();
    }
    let boxed_imports = slots_read_as_cells(&program.code, &program.functions);
    for (index, binding) in module.imports.iter().enumerate() {
        let slot = binding.local_slot;
        writeln!(output, "    locals[{slot}] = imports[{index}];").unwrap();
        // The exporter boxes only what can change, so an immutable export arrives as a plain
        // value where the body expects a cell. Wrapping it here is the same thing
        // `execute_module` does on the interpreter side, and for the same reason: the body has
        // one way to read an import and the linker has two ways to deliver one.
        if boxed_imports.contains(&slot) {
            writeln!(
                output,
                "    if (locals[{slot}].tag != KILN_VALUE_CELL && kiln_c99_make_cell(&locals[{slot}]) != KILN_C99_OK) return KILN_C99_OUT_OF_MEMORY;"
            )
            .unwrap();
        }
    }
    let (guarded_loads, guarded_slots) = compute_guarded_loads_and_slots(&program.code);
    let emitter = Emitter {
        constants: &program.constants,
        frame_stacks: &plan.functions,
        functions: &program.functions,
        strict_functions: &strict_functions,
        source: &program.source,
        symbol,
        diagnostic_site,
        callables,
        handlers: handler_targets(&program.code),
        site: ReturnSite::Entry {
            exports: Some(&module.exports),
            commonjs,
        },
        guarded: guarded_loads,
        guarded_slots,
        known_number_binaries: infer_program_types(program).1,
        current_func: None,
    };
    for index in 0..program.code.len() {
        writeln!(output, "kiln_label_{index}:;").unwrap();
        emit_instruction(&mut output, &program.code[index], index, &emitter);
    }
    render_unwind(&mut output, &emitter.handlers, false, true);
    render_uncaught(&mut output);
    output.push_str("    goto kiln_refusal;\n");
    render_refusal(&mut output);
    output.push_str("}\n");
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
        render_module_realm_wrapper(&mut output, symbol);
    }
    output
}

fn render_runtime_module(
    module: &ModuleProgram,
    program_hash: u64,
    symbol: &str,
    body: &str,
) -> String {
    let mut output = String::with_capacity(HEADER.len() + body.len() + 1024);
    output.push_str(HEADER);
    output = output.replace(
        "/* KILN_NATIVE_CONSTRUCTOR_TABLE */",
        &render_native_constructor_table(),
    );
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
    render_namespace_wrapper(&mut output, symbol);
    render_module_realm_wrapper(&mut output, symbol);
    output
}

/// Where a `Return` lands.
///
/// The entry point's value crosses the embedding boundary and a function body's does not, which
/// is the difference between converting a returned function to text and handing back the closure
/// so the caller can call it.
struct Emitter<'a> {
    constants: &'a [Constant],
    /// How deep each compiled body's operand stack can get, so a generator over it can allocate
    /// one that never overflows.
    frame_stacks: &'a [usize],
    /// The handler labels this body can unwind to, empty when it catches nothing. A body that
    /// catches nothing propagates a throw by returning it, which needs no machinery at all.
    handlers: Vec<u32>,
    functions: &'a [FunctionProto],
    /// Whether `functions[i]` is strict code, in the same order.
    ///
    /// The native backend needs this for exactly one rule: `OrdinaryCallBindThis`, where a
    /// sloppy body called with no receiver gets the global object and a strict one gets what it
    /// was passed. The interpreter reads the same answer off its `Strictness` table; a closure
    /// this backend builds carries the bit instead, because a C function has no table to consult.
    /// A short table -- a fixture with no strictness of its own -- reads strict, which is the
    /// answer every ESM unit has and so leaves the emitted bytes of one unchanged.
    strict_functions: &'a [bool],
    /// The unit's source text, which each proto's span slices to give a function its `toString`.
    source: &'a str,
    symbol: &'a str,
    diagnostic_site: &'a str,
    /// Whether a value reaching the embedding boundary could be a callable and so has to be
    /// converted first. False for a scalar program, whose emitted bytes must not change.
    callables: bool,
    site: ReturnSite<'a>,
    /// The instruction indices of loads that can read an uninitialized slot, so a read of one is
    /// checked and every other read is not.
    guarded: HashSet<usize>,
    /// The slots that have at least one guarded load.
    guarded_slots: HashSet<u32>,
    /// The binary instruction indices whose operands are provably numbers.
    known_number_binaries: HashSet<usize>,
    /// The index of the function currently being rendered, if inside a function body.
    current_func: Option<usize>,
}

enum ReturnSite<'a> {
    Entry {
        exports: Option<&'a [kiln_ir::ExportBinding]>,
        commonjs: bool,
    },
    Body,
}

fn emit_instruction(
    output: &mut String,
    instruction: &Instruction,
    index: usize,
    emitter: &Emitter<'_>,
) {
    let Emitter {
        constants,
        frame_stacks,
        functions,
        strict_functions,
        source,
        symbol,
        diagnostic_site,
        callables,
        handlers,
        site,
        guarded,
        guarded_slots,
        known_number_binaries,
        current_func,
    } = emitter;
    // A public scalar instruction starts a fresh diagnostic attempt. Function bodies do not
    // clear this state: when one calls another, the deepest failing body owns the refusal
    // provenance and every caller preserves it while unwinding.
    if matches!(site, ReturnSite::Entry { exports: None, .. }) {
        output.push_str(
            "    kiln_c99_diagnostic_site = NULL; kiln_c99_diagnostic_operation = NULL; kiln_c99_diagnostic_instruction = 0; kiln_c99_diagnostic_depth = 0; kiln_c99_diagnostic_method = 0; kiln_c99_diagnostic_receiver_tag = 0; kiln_c99_diagnostic_has_method = 0; kiln_c99_diagnostic_detail = NULL; kiln_c99_diagnostic_function = NULL; kiln_c99_diagnostic_function_len = 0;\n",
        );
    }
    let before = output.len();
    let instruction_site = c_string(diagnostic_site);
    let instruction_operation = c_string(&instruction_diagnostic(instruction, constants));
    let direct_refusal = format!(
        "kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); return KILN_C99_CONTROL_ERROR;"
    );
    let control_refusal = format!(
        "if (status == KILN_C99_CONTROL_ERROR || status == KILN_C99_THROWN) {{ kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth);"
    );
    match instruction {
        // Rejected by `verify` before emission begins, so reaching here means the two passes
        // disagree about the instruction set -- a compiler bug, not a program error.
        Instruction::LoadNewTarget => {
            output.push_str("    stack[sp++] = kiln_c99_new_target;\n");
        }
        // `validate_supported_instruction` refuses this, so reaching here means verification and
        // emission disagree about the instruction set -- a compiler bug rather than a program
        // one, which is the same reading the arms above take.
        Instruction::DirectEval(_) => {
            unreachable!("verification rejects a direct `eval` before emission")
        }
        Instruction::DynamicImport => {
            unreachable!("verification rejects a dynamic `import()` before emission")
        }
        Instruction::ToObject | Instruction::WithHas(_) => {
            unreachable!("verification rejects a `with` statement before emission")
        }
        Instruction::NewRegExp { pattern, flags } => {
            let (Constant::String(source), Constant::String(letters)) =
                (&constants[*pattern as usize], &constants[*flags as usize])
            else {
                unreachable!("verification rejects a non-string pattern before emission")
            };
            // A fresh object every time the literal is evaluated, because `lastIndex` is mutable
            // and program-visible: two evaluations of one literal must not share a cursor.
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ int status = kiln_c99_new_regexp(&stack[sp], {symbol}_constant_{pattern}, {}, {symbol}_constant_{flags}, {}, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; ++sp; }}",
                source.len(),
                letters.len()
            )
            .unwrap();
        }
        // The object and the key are read once and used twice, which two `Dup`s cannot do
        // because they would reverse them.
        Instruction::Dup2 => output
            .push_str("    stack[sp] = stack[sp - 2]; stack[sp + 1] = stack[sp - 1]; sp += 2;\n"),
        Instruction::CallComputed { argc } => {
            // Not a `GetComputed` followed by a `Call`: that would discard the receiver, and
            // `obj[name]()` has to run the method against `obj`. The same reason `CallMethod`
            // is one instruction rather than two.
            let argc = usize::from(*argc);
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value method; kiln_value call_result; int status; kiln_value key = stack[sp - {argc} - 1]; kiln_value self_value = stack[sp - {argc} - 2]; kiln_value call_argbuf[({argc}) + 1]; for (size_t i = 0; i < (size_t){argc}; i++) {{ call_argbuf[i] = stack[sp - {argc} + i]; }} const kiln_value *call_args = {argc} == 0 ? NULL : call_argbuf; status = kiln_c99_get_computed(host, self_value, key, &method, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; status = kiln_c99_call(host, &call_result, method, self_value, call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = call_result; }}",
                argc + 2
            )
            .unwrap();
        }
        Instruction::ConstructSpread => {
            // `new f(...args)` is the same construction the fixed-arity `New` performs, so it
            // goes through the same path rather than repeating the allocate-run-override
            // sequence.
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value list = stack[--sp]; kiln_value callee = stack[--sp]; kiln_value built; kiln_array *given; int status; if (list.tag != KILN_VALUE_ARRAY) {{ {direct_refusal} }} given = kiln_c99_array_of(list); status = kiln_c99_construct(host, &built, callee, given->count == 0 ? NULL : given->elements, given->count, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp++] = built; }}"
            )
            .unwrap();
        }
        Instruction::GetPrivate(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string private name before emission")
            };
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value receiver = stack[sp - 1]; kiln_property *slot = kiln_c99_own_private(receiver, {symbol}_constant_{name}, {len}); if (slot == NULL) {{ int status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {len}, 0); {unwind} }} if (slot->accessor) {{ kiln_value getter = slot->getter; kiln_value produced; int status; if (getter.tag == KILN_VALUE_UNDEFINED) {{ status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {len}, 2); {unwind} }} status = kiln_c99_call(host, &produced, getter, receiver, NULL, 0, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = produced; }} else {{ stack[sp - 1] = slot->value; }} }}",
                len = text.len()
            )
            .unwrap();
        }
        Instruction::SetPrivate(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string private name before emission")
            };
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value member = stack[--sp]; kiln_value receiver = stack[sp - 1]; kiln_property *slot = kiln_c99_own_private(receiver, {symbol}_constant_{name}, {len}); if (slot == NULL) {{ int status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {len}, 1); {unwind} }} if (slot->accessor) {{ kiln_value setter = slot->setter; kiln_value produced; int status; if (setter.tag == KILN_VALUE_UNDEFINED) {{ status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {len}, 3); {unwind} }} status = kiln_c99_call(host, &produced, setter, receiver, &member, 1, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }} else if (!slot->writable) {{ int status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {len}, 4); {unwind} }} else {{ slot->value = member; }} stack[sp - 1] = member; }}",
                len = text.len()
            )
            .unwrap();
        }
        Instruction::ThrowReference(name) => {
            // A name that is not there. Emitted rather than refused, because a reference in a
            // branch that never runs is not an error in JavaScript and real bundles are full of
            // them -- feature detection mentions globals the host may not have.
            //
            // "Not there" is decided at *run* time, though, not here. The compiler reaches this
            // instruction for every name outside `kiln_ir::Builtin`, and the realm's global object
            // holds intrinsics that table does not name -- `Temporal` is one. The interpreter has
            // always looked there before throwing (see its `ThrowReference` arm); this backend did
            // not, which is why a fully implemented `Temporal` was still a ReferenceError under
            // `--native`. The lookup is a no-op for every name the table *does* carry, because
            // those never reach this instruction at all.
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string reference name before emission")
            };
            writeln!(
                output,
                "    {{ static const char kiln_missing_name_{index}[] = {}; static const char kiln_missing_{index}[] = {}; int found = 0; int status = kiln_c99_global_binding(&stack[sp], kiln_missing_name_{index}, {}, &found); if (status != KILN_C99_OK) return status; if (found) {{ ++sp; }} else {{ status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, kiln_missing_{index}); {} }} }}",
                c_string(text),
                c_string(&format!("{text} is not defined")),
                text.len(),
                unwind_or_return(handlers, site)
            )
            .unwrap();
        }
        Instruction::DeclareGlobalBinding { name, configurable } => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ int status = kiln_c99_declare_global({symbol}_constant_{name}, {}, {}); if (status != KILN_C99_OK) return status; }}",
                text.len(),
                i32::from(*configurable)
            )
            .unwrap();
        }
        Instruction::DeleteGlobalBinding(name) => {
            // The mirror of `StoreGlobalBinding`: an unqualified `delete` in sloppy code is the
            // global object environment record's `DeleteBinding`, which is that object's
            // `[[Delete]]`. Both engines have to answer it the same way, because
            // `x = 1; delete x; x` throws in one and reads `1` in the other otherwise.
            let unwind = unwind_or_return(handlers, site);
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value globals; int status = kiln_c99_global_object(&globals); if (status != KILN_C99_OK) return status; int answer; status = kiln_c99_delete(host, globals, {symbol}_constant_{name}, {}, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp++] = kiln_c99_boolean(answer); }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::LoadGlobalBindingOrUndefined(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value existing; int found = 0; int status = kiln_c99_global_binding(&existing, {symbol}_constant_{name}, {}, &found); if (status != KILN_C99_OK) return status; stack[sp++] = found ? existing : kiln_c99_undefined(); }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::RequireGlobalBinding(name) => {
            let unwind = unwind_or_return(handlers, site);
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value existing; int found = 0; int status = kiln_c99_global_binding(&existing, {symbol}_constant_{name}, {}, &found); if (status != KILN_C99_OK) return status; if (!found) {{ static const char kiln_unbound_{index}[] = {}; status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, kiln_unbound_{index}); {unwind} }} }}",
                text.len(),
                c_string(&format!("{text} is not defined"))
            )
            .unwrap();
        }
        Instruction::StoreGlobalBindingStrict(name) => {
            // The strict form: an existing global is written, and a name the realm does not have
            // is the unresolvable reference strict mode refuses. Both engines must decide it the
            // same way and at the same moment, which is run time.
            let unwind = unwind_or_return(handlers, site);
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value member = stack[--sp]; kiln_value existing; int found = 0; int status = kiln_c99_global_binding(&existing, {symbol}_constant_{name}, {len}, &found); if (status != KILN_C99_OK) return status; if (!found) {{ static const char kiln_unbound_{index}[] = {}; status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, kiln_unbound_{index}); {unwind} }} {{ kiln_value globals; status = kiln_c99_global_object(&globals); if (status != KILN_C99_OK) return status; status = kiln_c99_put_mode(host, globals, {symbol}_constant_{name}, {len}, member, 1, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }} }}",
                c_string(&format!("{text} is not defined")),
                len = text.len()
            )
            .unwrap();
        }
        Instruction::StoreGlobalBinding(name) => {
            // Sloppy code settles an unresolvable assignment against the global object, which
            // creates the property. The interpreter does the same thing through its own global
            // object; both engines have to, or a UMD header installs itself under one and throws
            // under the other -- a divergence worse than the shared refusal it replaced.
            let unwind = unwind_or_return(handlers, site);
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value member = stack[--sp]; kiln_value globals; int status = kiln_c99_global_object(&globals); if (status != KILN_C99_OK) return status; status = kiln_c99_put_mode(host, globals, {symbol}_constant_{name}, {}, member, 0, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::Const(constant) => match &constants[*constant as usize] {
            Constant::Undefined => output.push_str("    stack[sp++] = kiln_c99_undefined();\n"),
            Constant::Null => output.push_str("    stack[sp++] = kiln_c99_null();\n"),
            Constant::Boolean(value) => {
                writeln!(output, "    stack[sp++] = kiln_fast_boolean({});", i32::from(*value)).unwrap()
            }
            Constant::Number(bits) => {
                writeln!(output, "    stack[sp++] = kiln_fast_number_bits(UINT64_C({bits}));").unwrap()
            }
            Constant::String(value) => {
                writeln!(output, "    stack[sp++] = kiln_c99_value(KILN_VALUE_STRING, 0, {symbol}_constant_{constant}, {});", value.len()).unwrap()
            }
            Constant::BigInt(digits) => writeln!(
                output,
                "    {{ int status = kiln_c99_bigint_constant(&{symbol}_bigint_{constant}, {symbol}_constant_{constant}, {}, &stack[sp]); if (status != KILN_C99_OK) return status; ++sp; }}",
                digits.len()
            )
            .unwrap(),
        },
        // A cell is a shared box: the slot stops holding the value and starts holding a
        // pointer to it, so that a closure over the variable and the frame that declared it are
        // writing to the same place. Until closures existed here this was emitted as plain slot
        // access, which was sound only because nothing could capture.
        Instruction::NewCell(local) => {
            writeln!(
                output,
                "    {{ int status = kiln_c99_make_cell(&locals[{local}]); if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        Instruction::LoadCell(local) => {
            if guarded.contains(&index) {
                let unwind = unwind_or_return(handlers, site);
                writeln!(
                    output,
                    "    if (KILN_UNLIKELY(kiln_c99_cell_of(locals[{local}])->tag == KILN_VALUE_UNINITIALIZED)) {{ int status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, \"cannot access a binding before its declaration\"); if (KILN_UNLIKELY(status == KILN_C99_THROWN)) {unwind} if (KILN_UNLIKELY(status != KILN_C99_OK)) return status; }}"
                )
                .unwrap();
            }
            writeln!(output, "    stack[sp++] = *kiln_c99_cell_of(locals[{local}]);").unwrap();
        }
        Instruction::StoreCell(local) => {
            writeln!(output, "    *kiln_c99_cell_of(locals[{local}]) = stack[--sp];").unwrap();
        }
        // A `let` or `const` slot is in the temporal dead zone until its declaration runs.
        // Reading one is an error rather than `undefined`, and only the slots a program marks
        // are ever tested -- the check would otherwise be a branch on every local read for a
        // state almost no program reaches.
        Instruction::MarkUninitialized(local) => {
            // Inlined rather than `kiln_c99_mark_uninitialized(&locals[N])`. Taking the address
            // of a slot and passing it to an opaque function makes the whole `locals[]` array
            // address-escaped, and an escaped array cannot be promoted to registers *anywhere in
            // the function* -- so one TDZ mark in the prologue costs every loop in the body. The
            // body it replaces is two lines and is kept verbatim.
            // Furthermore, if dataflow proves no load can observe this slot uninitialized,
            // omitting the write avoids thrashing memory in tight loop bodies.
            if guarded_slots.contains(local) {
                writeln!(output, "    if (locals[{local}].tag == KILN_VALUE_CELL) {{ *kiln_c99_cell_of(locals[{local}]) = kiln_c99_uninitialized(); }} else {{ locals[{local}] = kiln_c99_uninitialized(); }}").unwrap();
            }
        }
        // An upvalue slot always holds a cell -- that is what capturing means -- but the cell can
        // still be in its owner's temporal dead zone. The owner may be a module or an enclosing
        // function, so this body cannot use its own `MarkUninitialized` set to decide whether to
        // check. Reading the marker is cheap compared with the pointer chase and keeps captured
        // bindings in parity with `LoadCell` and the interpreter.
        Instruction::LoadUpvalue(upvalue) => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    if (kiln_c99_cell_of(upvalues[{upvalue}])->tag == KILN_VALUE_UNINITIALIZED) {{ int status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, \"cannot access a binding before its declaration\"); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
            writeln!(output, "    stack[sp++] = *kiln_c99_cell_of(upvalues[{upvalue}]);").unwrap();
        }
        Instruction::StoreUpvalue(upvalue) => {
            writeln!(output, "    *kiln_c99_cell_of(upvalues[{upvalue}]) = stack[--sp];").unwrap();
        }
        Instruction::MakeFunction(proto) => {
            let target = &functions[*proto as usize];
            let captures = &target.upvalues;
            // A console prints `[Function: name]`, and an anonymous function is a distinct
            // answer rather than a missing one.
            let (name, name_len) = match &target.name {
                Some(text) => (format!("{symbol}_name_{proto}"), text.len()),
                None => ("NULL".to_owned(), 0),
            };
            // `Function.prototype.toString` answers with this slice of the unit's source. A span
            // the text cannot hold is not narrowed to fit -- it is dropped, and the closure says
            // it has no source, which is the same fallback the interpreter takes.
            let span = target.span;
            let (source, source_len) = if source.is_empty()
                || span.end as usize > source.len()
                || span.start > span.end
            {
                ("NULL".to_owned(), 0usize)
            } else {
                (
                    format!("{symbol}_source + {}", span.start),
                    (span.end - span.start) as usize,
                )
            };
            // A generator's frame is sized here rather than at the call, because this is where
            // the body's own counts are in scope.
            let generator = i32::from(target.generator || target.asynchronous);
            let asynchronous = i32::from(target.asynchronous);
            let arrow = i32::from(target.arrow);
            let async_generator = i32::from(target.async_generator);
            // The one thing this backend does with strictness beyond the flags already baked
            // into `SetPropertyStrict` and friends: it decides the receiver a bare call gets.
            let strict = i32::from(
                strict_functions
                    .get(*proto as usize)
                    .copied()
                    .unwrap_or(true),
            );
            // `f.length` is how many arguments a caller *must* supply, which is the parameters
            // before the first default and before a rest -- not how many slots the frame has.
            // `(a, b = 1) => a` has two slots and a length of one, and reporting two said the
            // second argument was required. `bind` subtracts the ones it fixed from this.
            let arity = target.declared_length;
            let frame_locals = target.local_count.max(1);
            let frame_stack = frame_stacks[*proto as usize].max(1);
            // One handler slot per instruction: a body cannot register more handlers than it has
            // instructions, which is the same bound an automatic frame uses.
            let frame_handlers = target.code.len().max(1);
            // Non-zero only where the binding runs at the call; the runtime needs no other test,
            // and the value is the resume position that seam leaves behind.
            let prologue_resume = if binds_at_call(target) {
                target.code.len()
            } else {
                0
            };
            // Every function body receives its effective receiver. An arrow snapshots that
            // value now; an ordinary closure carries the same field but never consults it.
            // Module entry has no receiver and therefore captures `undefined` explicitly.
            let defining_receiver = if target.arrow && matches!(site, ReturnSite::Body) {
                "receiver"
            } else {
                "kiln_c99_undefined()"
            };
            let diagnostic_source = c_string(diagnostic_site);
            output.push_str("    {\n        int status;\n");
            if captures.is_empty() {
                writeln!(
                    output,
                    "        status = kiln_c99_make_closure(&stack[sp], {symbol}_fn_{proto}, {name}, {name_len}, {source}, {source_len}, {diagnostic_source}, UINT32_C({proto}), UINT32_C({arity}), {generator}, {asynchronous}, {async_generator}, {arrow}, {strict}, {defining_receiver}, kiln_c99_new_target, UINT32_C({frame_locals}), UINT32_C({frame_stack}), UINT32_C({frame_handlers}), UINT32_C({prologue_resume}), NULL, 0);"
                )
                .unwrap();
            } else {
                writeln!(output, "        kiln_value captured[{}];", captures.len()).unwrap();
                for (position, upvalue) in captures.iter().enumerate() {
                    // The box is copied by pointer, which is the point: two closures over one
                    // variable have to observe each other's writes.
                    match upvalue {
                        Upvalue::ParentLocal(slot) => {
                            writeln!(output, "        captured[{position}] = locals[{slot}];")
                        }
                        Upvalue::ParentUpvalue(slot) => {
                            writeln!(output, "        captured[{position}] = upvalues[{slot}];")
                        }
                    }
                    .unwrap();
                }
                writeln!(
                    output,
                    "        status = kiln_c99_make_closure(&stack[sp], {symbol}_fn_{proto}, {name}, {name_len}, {source}, {source_len}, {diagnostic_source}, UINT32_C({proto}), UINT32_C({arity}), {generator}, {asynchronous}, {async_generator}, {arrow}, {strict}, {defining_receiver}, kiln_c99_new_target, UINT32_C({frame_locals}), UINT32_C({frame_stack}), UINT32_C({frame_handlers}), UINT32_C({prologue_resume}), captured, UINT32_C({}));",
                    captures.len()
                )
                .unwrap();
            }
            if proto_publishes_frame(target) {
                output.push_str("        if (status != KILN_C99_OK) return status;\n        ++sp;\n    }\n");
            } else {
                output.push_str("        if (status != KILN_C99_OK) return status;\n        kiln_c99_closure_of(stack[sp])->publishes_frame = 0;\n        ++sp;\n    }\n");
            }
        }
        Instruction::Call { argc } => {
            let argc = usize::from(*argc);
            let unwind = unwind_or_return(handlers, site);
            let direct_call = if let Some(func_idx) = *current_func {
                let func_uses_this = functions.get(func_idx).is_some_and(|proto| {
                    proto.code.iter().any(|inst| matches!(inst, Instruction::LoadThis))
                });
                let this_cond = if !func_uses_this {
                    "1"
                } else {
                    "(plain->arrow || plain->strict)"
                };
                format!(
                    "if (KILN_LIKELY(plain != NULL && plain->body == {symbol}_fn_{func_idx} && {this_cond} && !plain->generator && !plain->publishes_frame && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH)) {{ status = {symbol}_fn_{func_idx}(host, &call_result, plain->upvalues, plain->arrow ? plain->captured_this : kiln_fast_undefined(), call_args, (size_t){argc}, depth + 1, thrown, NULL); }} else "
                )
            } else {
                String::new()
            };
            writeln!(
                output,
                "    {{ kiln_value call_result = kiln_fast_undefined(); kiln_value callee = stack[sp - {argc} - 1]; int status; kiln_value call_argbuf[({argc}) + 1]; for (size_t i = 0; i < (size_t){argc}; i++) {{ call_argbuf[i] = stack[sp - {argc} + i]; }} const kiln_value *call_args = {argc} == 0 ? NULL : call_argbuf; {{ kiln_closure *plain = callee.tag == KILN_VALUE_FUNCTION ? kiln_c99_closure_of(callee) : NULL; {direct_call}if (KILN_LIKELY(plain != NULL && (plain->arrow || plain->strict) && !plain->generator && !plain->publishes_frame && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH)) {{ status = plain->body(host, &call_result, plain->upvalues, plain->arrow ? plain->captured_this : kiln_fast_undefined(), call_args, (size_t){argc}, depth + 1, thrown, NULL); }} else if (plain != NULL && (plain->arrow || plain->strict) && !plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ status = kiln_c99_invoke_closure(host, &call_result, plain, plain->arrow ? plain->captured_this : kiln_c99_undefined(), call_args, (size_t){argc}, depth, thrown, NULL, 0); }} else {{ status = kiln_c99_call(host, &call_result, callee, kiln_c99_undefined(), call_args, (size_t){argc}, depth, thrown); }} }} if (status == KILN_C99_CONTROL_ERROR && call_result.tag == KILN_VALUE_STRING) {{ *result = call_result; return status; }} if (status == KILN_C99_CONTROL_ERROR && kiln_c99_diagnostic_receiver_tag == 0) {{ kiln_c99_diagnostic_receiver_tag = callee.tag; }} if (status == KILN_C99_CONTROL_ERROR && kiln_c99_diagnostic_site == NULL && kiln_c99_diagnostic_operation == NULL && kiln_c99_diagnostic_detail == NULL) {{ kiln_c99_diagnostic_detail = \"Call callee is unsupported by the native callable model\"; }} if (status == KILN_C99_CONTROL_ERROR) kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); if (KILN_UNLIKELY(status == KILN_C99_THROWN)) {unwind} if (KILN_UNLIKELY(status != KILN_C99_OK)) return status; sp -= (size_t){}; stack[sp++] = call_result; }}",
                argc + 1
            )
            .unwrap();
        }
        Instruction::LoadLocal(local) => {
            if guarded.contains(&index) {
                let unwind = unwind_or_return(handlers, site);
                writeln!(
                    output,
                    "    if (KILN_UNLIKELY(locals[{local}].tag == KILN_VALUE_UNINITIALIZED)) {{ int status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, \"cannot access a binding before its declaration\"); if (KILN_UNLIKELY(status == KILN_C99_THROWN)) {unwind} if (KILN_UNLIKELY(status != KILN_C99_OK)) return status; }}"
                )
                .unwrap();
            }
            writeln!(output, "    stack[sp++] = locals[{local}];").unwrap();
        }
        Instruction::StoreLocal(local) => writeln!(output, "    locals[{local}] = stack[--sp];").unwrap(),
        // The keys are on the stack with their values, so the object is filled from the stack
        // and only then replaces it -- a key that failed to convert must not leave a half-built
        // object where the program can see it.
        Instruction::NewObject { count } => {
            let unwind = unwind_or_return(handlers, site);
            let count = usize::try_from(*count).unwrap();
            output.push_str("    {\n        kiln_value built; size_t pair; int status;\n");
            if count > 0 {
                // `count` is pairs, not final properties -- a literal with a duplicate key
                // (`{ a: 1, a: 2 }`) asks for one more slot than it ends up filling, which
                // `kiln_c99_new_object_with_capacity` already documents as a hint rather than a
                // promise. Sizing once here is what a template-filling caller (this loop is
                // exactly that: a fixed member count known before the first store) should do
                // instead of paying `kiln_c99_reserve`'s 4 -> 8 -> ... walk one `define_computed`
                // at a time.
                writeln!(
                    output,
                    "        status = kiln_c99_new_object_with_capacity(&built, (size_t){count});"
                )
                .unwrap();
            } else {
                output.push_str("        status = kiln_c99_new_object(&built);\n");
            }
            output.push_str("        if (status != KILN_C99_OK) return status;\n");
            if count > 0 {
                writeln!(
                    output,
                    "        for (pair = 0; pair < (size_t){count}; ++pair) {{ kiln_value key = stack[sp - (size_t){count} * 2 + pair * 2]; kiln_value member = stack[sp - (size_t){count} * 2 + pair * 2 + 1]; status = kiln_c99_define_computed(built, key, member); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
                )
                .unwrap();
                writeln!(output, "        sp -= (size_t){count} * 2;").unwrap();
            } else {
                output.push_str("        (void)pair;\n");
            }
            output.push_str("        stack[sp++] = built;\n    }\n");
        }
        Instruction::NewArray { count } => {
            let count = usize::try_from(*count).unwrap();
            output.push_str("    {\n        kiln_value built; size_t element; int status;\n");
            output.push_str("        status = kiln_c99_new_array(&built);\n");
            output.push_str("        if (status != KILN_C99_OK) return status;\n");
            if count > 0 {
                writeln!(
                    output,
                    "        for (element = 0; element < (size_t){count}; ++element) {{ status = kiln_c99_array_push(built, stack[sp - (size_t){count} + element]); if (status != KILN_C99_OK) return status; }}"
                )
                .unwrap();
                writeln!(output, "        sp -= (size_t){count};").unwrap();
            } else {
                output.push_str("        (void)element;\n");
            }
            output.push_str("        stack[sp++] = built;\n    }\n");
        }
        Instruction::GetProperty(name) => {
            let unwind = unwind_or_return(handlers, site);
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; kiln_value read; kiln_value target = stack[sp - 1]; int status = kiln_c99_get_site(host, target, {symbol}_key_{name}, {}, &read, depth, thrown, &site); if (status == KILN_C99_CONTROL_ERROR) {{ if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = target.tag; if (target.tag == KILN_VALUE_BUILTIN && kiln_c99_diagnostic_function == NULL) {{ const char *named = kiln_c99_builtin_name(target.bits); if (named != NULL) {{ kiln_c99_diagnostic_function = named; kiln_c99_diagnostic_function_len = strlen(named); }} }} if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = \"GetProperty lookup refused\"; }} if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = read; }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::SetProperty(name) => {
            let unwind = unwind_or_return(handlers, site);
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            // The written value stays on the stack, which is what makes `a = b.c = d` work.
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; kiln_value member = stack[--sp]; int status = kiln_c99_put_site(host, stack[sp - 1], {symbol}_key_{name}, {}, member, 0, depth, thrown, &site); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = member; }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::SetPropertyStrict { name, strict } => {
            let unwind = unwind_or_return(handlers, site);
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; kiln_value member = stack[--sp]; int status = kiln_c99_put_site(host, stack[sp - 1], {symbol}_key_{name}, {}, member, {}, depth, thrown, &site); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = member; }}",
                text.len(),
                i32::from(*strict)
            )
            .unwrap();
        }
        Instruction::GetComputed => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value key = stack[--sp]; kiln_value read; int status = kiln_c99_get_computed(host, stack[sp - 1], key, &read, depth, thrown); if (status == KILN_C99_CONTROL_ERROR) {{ if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = stack[sp - 1].tag; if (kiln_c99_diagnostic_function == NULL && kiln_c99_diagnostic_detail != kiln_c99_diagnostic_compose) {{ const char *named = stack[sp - 1].tag == KILN_VALUE_BUILTIN ? kiln_c99_builtin_name(stack[sp - 1].bits) : NULL; if (key.tag == KILN_VALUE_STRING && key.ptr != NULL) {{ int written = named != NULL ? snprintf(kiln_c99_diagnostic_compose, sizeof kiln_c99_diagnostic_compose, \"%s.%.*s\", named, (int)key.len, key.ptr) : snprintf(kiln_c99_diagnostic_compose, sizeof kiln_c99_diagnostic_compose, \"%.*s\", (int)key.len, key.ptr); if (written > 0) {{ kiln_c99_diagnostic_function = kiln_c99_diagnostic_compose; kiln_c99_diagnostic_function_len = (size_t)written < sizeof kiln_c99_diagnostic_compose ? (size_t)written : sizeof kiln_c99_diagnostic_compose - 1; }} }} else if (named != NULL) {{ kiln_c99_diagnostic_function = named; kiln_c99_diagnostic_function_len = strlen(named); }} }} if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = \"GetComputed lookup refused\"; }} if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = read; }}"
            )
            .unwrap();
        }
        Instruction::SetComputed => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value member = stack[--sp]; kiln_value key = stack[--sp]; int status = kiln_c99_set_computed(host, stack[sp - 1], key, member, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = member; }}"
            )
            .unwrap();
        }
        Instruction::SetComputedStrict { strict } => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value member = stack[--sp]; kiln_value key = stack[--sp]; int status = kiln_c99_set_computed_mode(host, stack[sp - 1], key, member, {}, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = member; }}",
                i32::from(*strict)
            )
            .unwrap();
        }
        Instruction::LoadThis => output.push_str("    stack[sp++] = receiver;\n"),
        // Module scope is one array for the whole module, so a read from a function body names
        // it directly rather than reaching for a frame that may no longer exist.
        Instruction::LoadGlobal(slot) => {
            if guarded.contains(&index) {
                let unwind = unwind_or_return(handlers, site);
                writeln!(
                    output,
                    "    if (KILN_UNLIKELY({symbol}_globals[{slot}].tag == KILN_VALUE_UNINITIALIZED)) {{ int status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, \"cannot access a binding before its declaration\"); if (KILN_UNLIKELY(status == KILN_C99_THROWN)) {unwind} if (KILN_UNLIKELY(status != KILN_C99_OK)) return status; }}"
                )
                .unwrap();
            }
            writeln!(output, "    stack[sp++] = {symbol}_globals[{slot}];").unwrap();
        }
        Instruction::StoreGlobal(slot) => {
            writeln!(output, "    {symbol}_globals[{slot}] = stack[--sp];").unwrap();
        }
        Instruction::Binary(BinaryOp::InstanceOf) => {
            output.push_str(
                "    { kiln_value constructor = stack[--sp]; int answer; int status = kiln_c99_instance_of(stack[sp - 1], constructor, &answer); if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_c99_boolean(answer); }\n",
            );
        }
        // Both halves of `super` are an ordinary call with `this` threaded through: the parent
        // initialises the instance that already exists rather than building one, which is the
        // whole difference between `super()` and `new Parent()`.
        Instruction::SuperCall { argc } => {
            let argc = usize::from(*argc);
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value produced; int status; kiln_value call_argbuf[({argc}) + 1]; for (size_t i = 0; i < (size_t){argc}; i++) {{ call_argbuf[i] = stack[sp - {argc} + i]; }} const kiln_value *call_args = {argc} == 0 ? NULL : call_argbuf; status = kiln_c99_super_call(host, &produced, stack[sp - {argc} - 1], receiver, call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; status = kiln_c99_adopt_super(&receiver, produced); if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = produced; }}",
                argc + 1
            )
            .unwrap();
        }
        // A derived class with no constructor behaves as `constructor(...args) { super(...args) }`.
        // Its arity is not known where it is synthesized, so the forwarding happens here from
        // whatever the frame was actually given.
        Instruction::SuperForward => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value produced; int status = kiln_c99_super_call(host, &produced, stack[sp - 1], receiver, argc == 0 ? NULL : args, argc, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; status = kiln_c99_adopt_super(&receiver, produced); if (status != KILN_C99_OK) return status; stack[sp - 1] = produced; }}"
            )
            .unwrap();
        }
        Instruction::New { argc } => {
            let argc = usize::from(*argc);
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value built; int status; kiln_value call_argbuf[({argc}) + 1]; for (size_t i = 0; i < (size_t){argc}; i++) {{ call_argbuf[i] = stack[sp - {argc} + i]; }} const kiln_value *call_args = {argc} == 0 ? NULL : call_argbuf; status = kiln_c99_construct(host, &built, stack[sp - {argc} - 1], call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_CONTROL_ERROR) {{ if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = stack[sp - {argc} - 1].tag; if (stack[sp - {argc} - 1].tag == KILN_VALUE_BUILTIN && kiln_c99_diagnostic_function == NULL) {{ const char *named = kiln_c99_builtin_name(stack[sp - {argc} - 1].bits); if (named != NULL) {{ kiln_c99_diagnostic_function = named; kiln_c99_diagnostic_function_len = strlen(named); }} }} if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = \"New target is unsupported by the native constructor model\"; }} if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = built; }}",
                argc + 1
            )
            .unwrap();
        }
        Instruction::DefineAccessor {
            setter,
            enumerable,
        } => {
            let half = if *setter { "setter" } else { "getter" };
            let other = if *setter { "getter" } else { "setter" };
            let prefix = if *setter { "set" } else { "get" };
            let enumerable = i32::from(*enumerable);
            writeln!(
                output,
                "    {{ kiln_value {half} = stack[--sp]; kiln_value key = stack[--sp]; kiln_value {other} = kiln_c99_undefined(); kiln_value name = kiln_c99_property_key(key); int status; if (name.tag != KILN_VALUE_STRING) {{ {direct_refusal} }} status = kiln_c99_set_function_name({half}, key, \"{prefix}\"); if (status != KILN_C99_OK) return status; status = kiln_c99_define_accessor(stack[sp - 1], name.ptr, name.len, getter, setter, {enumerable}); if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        // A private element is installed with its kind rather than as a slot holding a function,
        // and on the *object* rather than on a prototype: the presence of the name is the brand,
        // so an instance element belongs to each instance.
        Instruction::DefinePrivate { name, kind } => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string private name before emission")
            };
            let (tag, prefix) = match kind {
                kiln_ir::PrivateKind::Method => (0, "NULL"),
                kiln_ir::PrivateKind::Getter => (1, "\"get\""),
                kiln_ir::PrivateKind::Setter => (2, "\"set\""),
            };
            writeln!(
                output,
                "    {{ kiln_value function = stack[--sp]; kiln_value key = kiln_c99_value(KILN_VALUE_STRING, 0, {symbol}_constant_{name}, {len}); if (kiln_c99_set_function_name(function, key, {prefix}) != KILN_C99_OK) {{ {direct_refusal} }} if (kiln_c99_define_private(stack[sp - 1], {symbol}_constant_{name}, {len}, function, {tag}) != KILN_C99_OK) {{ {direct_refusal} }} }}",
                len = text.len()
            )
            .unwrap();
        }
        // A class's methods are not enumerated, which is the whole difference from the
        // `SetProperty` an object literal's methods lower to.
        Instruction::DefineMethod => {
            writeln!(
                output,
                "    {{ kiln_value method = stack[--sp]; kiln_value key = stack[--sp]; kiln_value name = kiln_c99_property_key(key); if (name.tag != KILN_VALUE_STRING) {{ {direct_refusal} }} if (kiln_c99_set_function_name(method, key, NULL) != KILN_C99_OK) {{ {direct_refusal} }} if (kiln_c99_define_method(stack[sp - 1], name.ptr, name.len, method) != KILN_C99_OK) {{ {direct_refusal} }} }}"
            )
            .unwrap();
        }
        // `in` asks whether the property exists, not what it produces, so unlike a read it does
        // not call an accessor.
        //
        // The key is resolved the way a *read* resolves one, not by converting to text: a symbol
        // keys its reserved slot, so `Symbol.iterator in x` has to ask about `@@iterator` rather
        // than about the property literally spelled "Symbol(Symbol.iterator)", which nothing has.
        Instruction::Binary(BinaryOp::In) => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value container = stack[--sp]; int answer; int status = kiln_c99_has_computed(host, container, stack[sp - 1], &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_c99_boolean(answer); }}"
            )
            .unwrap();
        }
        Instruction::DeleteProperty(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ int answer; int status = kiln_c99_delete(host, stack[sp - 1], {symbol}_key_{name}, {}, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_c99_boolean(answer); }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::DeletePropertyStrict { name, strict } => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ int answer; int status = kiln_c99_delete_or_throw(host, stack[sp - 1], {symbol}_key_{name}, {}, {}, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_c99_boolean(answer); }}",
                text.len(),
                i32::from(*strict)
            )
            .unwrap();
        }
        Instruction::DeleteComputed => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value key = stack[--sp]; int answer; int status = kiln_c99_delete_computed(host, stack[sp - 1], key, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_c99_boolean(answer); }}"
            )
            .unwrap();
        }
        Instruction::DeleteComputedStrict { strict } => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value key = stack[--sp]; int answer; int status = kiln_c99_delete_computed_or_throw(host, stack[sp - 1], key, {}, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_c99_boolean(answer); }}",
                i32::from(*strict)
            )
            .unwrap();
        }
        // `arguments` is every argument this frame was given, as an array. A real one is an
        // exotic object that aliases the parameters; a copy is what almost every use of it wants,
        // and the aliasing was removed from the language in strict mode anyway.
        // Not `args` and `argc`: inside an arrow those are the arrow's own, and `arguments`
        // there is the enclosing function's -- a different list entirely.
        Instruction::LoadArguments => {
            output.push_str(
                "    { kiln_value given; int status = kiln_c99_visible_arguments(&given); if (status != KILN_C99_OK) return status; stack[sp++] = given; }\n",
            );
        }
        // `...rest` is every argument past the named parameters, as an array. A generator's
        // arguments were copied into its frame when it was built, so there are none here.
        Instruction::RestArguments(from) => {
            writeln!(
                output,
                "    {{ kiln_value rest; size_t extra; int status = kiln_c99_new_array(&rest); if (status != KILN_C99_OK) return status; for (extra = (size_t){from}; extra < argc; ++extra) {{ status = kiln_c99_array_push(rest, args[extra]); if (status != KILN_C99_OK) return status; }} stack[sp++] = rest; }}"
            )
            .unwrap();
        }
        // The marker for an element a literal left out. It never escapes into an expression:
        // `NewArray` is the only thing that consumes it.
        Instruction::PushHole => output.push_str("    stack[sp++] = kiln_c99_hole();\n"),
        Instruction::EnumerateKeys => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value keys; int status = kiln_c99_enumerate_keys(host, stack[sp - 1], &keys, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = keys; }}"
            )
            .unwrap();
        }
        // The arity comes from an array the program built, so the arguments are read out of it
        // rather than off the stack.
        Instruction::Apply => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value list = stack[--sp]; kiln_value callee = stack[--sp]; kiln_value self_value = stack[sp - 1]; kiln_value call_result; kiln_array *given; int status; if (list.tag != KILN_VALUE_ARRAY) {{ {direct_refusal} }} given = kiln_c99_array_of(list); status = kiln_c99_call(host, &call_result, callee, self_value, given->count == 0 ? NULL : given->elements, given->count, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = call_result; }}"
            )
            .unwrap();
        }
        // `Apply`'s runtime argument list, but against the instance being built and keeping the
        // `new.target` of the call that is building it -- the two things `super()` adds.
        Instruction::SuperApply => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value list = stack[--sp]; kiln_value parent = stack[--sp]; kiln_value call_result; kiln_array *given; int status; if (list.tag != KILN_VALUE_ARRAY) {{ {direct_refusal} }} given = kiln_c99_array_of(list); status = kiln_c99_super_call(host, &call_result, parent, receiver, given->count == 0 ? NULL : given->elements, given->count, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; status = kiln_c99_adopt_super(&receiver, call_result); if (status != KILN_C99_OK) return status; stack[sp++] = call_result; }}"
            )
            .unwrap();
        }
        Instruction::SpreadInto => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value source = stack[--sp]; int status = kiln_c99_iterate(host, stack[sp - 1], source, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        Instruction::IterableToArray => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value built; int status = kiln_c99_new_array(&built); if (status != KILN_C99_OK) return status; status = kiln_c99_iterate(host, built, stack[sp - 1], depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = built; }}"
            )
            .unwrap();
        }
        Instruction::SpreadObject => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value source = stack[--sp]; int status = kiln_c99_spread_object(host, stack[sp - 1], source, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        // `class D extends C` links the two prototype objects, which is what makes an instance
        // of `D` find a method of `C`.
        Instruction::SetPrototype => {
            output.push_str(
                "    { kiln_value prototype = stack[--sp]; int status = kiln_c99_set_prototype(stack[sp - 1], prototype); if (status != KILN_C99_OK) return status; }\n",
            );
        }
        // `super.x` reads the link the method's home object carries *now*, which is the same
        // question `Object.getPrototypeOf` answers -- so it is asked through the same
        // implementation rather than a second one that could drift from it.
        // The receiver is the frame's own, not the prototype the name was found on -- which is
        // the one thing that separates `super.x` from an ordinary read of the same name.
        Instruction::LoadSuperProperty(name) => {
            let unwind = unwind_or_return(handlers, site);
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value read; int status = kiln_c99_get_with_receiver(host, stack[sp - 1], kiln_c99_value(KILN_VALUE_STRING, 0, {symbol}_key_{name}, {}), receiver, &read, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = read; }}",
                text.len()
            )
            .unwrap();
        }
        // The computed spelling. The key was pushed first, so it is the value the read leaves
        // its answer in place of.
        Instruction::LoadSuperElement => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value prototype = stack[--sp]; kiln_value read; int status = kiln_c99_get_with_receiver(host, prototype, stack[sp - 1], receiver, &read, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = read; }}"
            )
            .unwrap();
        }
        Instruction::LoadPrototype => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value subject = stack[sp - 1]; int status = kiln_c99_invoke_method(host, &stack[sp - 1], KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF, kiln_c99_undefined(), &subject, 1, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        // The stack height is recorded with the handler, because a `catch` starts from the
        // height the `try` began at however deep the expression that threw had got.
        Instruction::PushHandler(target) => {
            writeln!(
                output,
                "    kiln_handlers[kiln_handler_top] = {target}; kiln_handler_stack[kiln_handler_top] = sp; ++kiln_handler_top;"
            )
            .unwrap();
        }
        Instruction::PopHandler => output.push_str("    --kiln_handler_top;\n"),
        Instruction::Yield | Instruction::Await => {
            writeln!(
                output,
                "    frame->sp = sp - 1; frame->resume = {index}; frame->awaiting = {}; *result = stack[sp - 1]; return KILN_C99_YIELDED;",
                i32::from(matches!(instruction, Instruction::Await))
            )
            .unwrap();
            // Where `next` comes back to. What the caller sent becomes the value of the `yield`
            // expression, which is why the height here is one more than it was at the suspension.
            writeln!(output, "kiln_resume_{index}:;").unwrap();
            output.push_str("    stack[sp++] = frame->sent;\n");
        }
        Instruction::Throw => {
            output.push_str("    *thrown = stack[--sp];\n");
            // Nothing here can catch it, so it leaves immediately -- as a status from a body,
            // and as text through the uncaught epilogue from the entry point. Spelled out rather
            // than shared with `unwind_or_return`, whose propagating arm returns the `status` a
            // failed call left behind and there is no call here to have left one.
            output.push_str(if !handlers.is_empty() {
                "    goto kiln_unwind;\n"
            } else if matches!(site, ReturnSite::Entry { .. }) {
                "    goto kiln_uncaught;\n"
            } else {
                "    return KILN_C99_THROWN;\n"
            });
        }
        // Not a `GetProperty` followed by a `Call`: that would discard the receiver, and the
        // read has to yield the *unbound* function so `a.f === a.f` holds. The receiver stays on
        // the stack until the call, which is what lets both be true at once.
        Instruction::CallMethod { name, argc } => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string method name before emission")
            };
            let argc = usize::from(*argc);
            let unwind = unwind_or_return(handlers, site);
            let fast_method = match text.as_str() {
                "set" => format!("if (KILN_LIKELY(self_value.tag == KILN_VALUE_COLLECTION)) {{ kiln_collection *col = (kiln_collection *)(uintptr_t)self_value.bits; if (col->is_map && col->properties.tag == KILN_VALUE_UNDEFINED) {{ status = kiln_c99_invoke_method(host, &call_result, KILN_C99_METHOD_COLLECTION_SET, self_value, call_args, (size_t){argc}, depth, thrown); if (KILN_LIKELY(status == KILN_C99_OK)) goto kiln_method_done_{index}; }} }} "),
                "get" => format!("if (KILN_LIKELY(self_value.tag == KILN_VALUE_COLLECTION)) {{ kiln_collection *col = (kiln_collection *)(uintptr_t)self_value.bits; if (col->is_map && col->properties.tag == KILN_VALUE_UNDEFINED) {{ status = kiln_c99_invoke_method(host, &call_result, KILN_C99_METHOD_COLLECTION_GET, self_value, call_args, (size_t){argc}, depth, thrown); if (KILN_LIKELY(status == KILN_C99_OK)) goto kiln_method_done_{index}; }} }} "),
                "has" => format!("if (KILN_LIKELY(self_value.tag == KILN_VALUE_COLLECTION)) {{ kiln_collection *col = (kiln_collection *)(uintptr_t)self_value.bits; if (col->properties.tag == KILN_VALUE_UNDEFINED) {{ status = kiln_c99_invoke_method(host, &call_result, KILN_C99_METHOD_COLLECTION_HAS, self_value, call_args, (size_t){argc}, depth, thrown); if (KILN_LIKELY(status == KILN_C99_OK)) goto kiln_method_done_{index}; }} }} "),
                "add" => format!("if (KILN_LIKELY(self_value.tag == KILN_VALUE_COLLECTION)) {{ kiln_collection *col = (kiln_collection *)(uintptr_t)self_value.bits; if (!col->is_map && col->properties.tag == KILN_VALUE_UNDEFINED) {{ status = kiln_c99_invoke_method(host, &call_result, KILN_C99_METHOD_COLLECTION_ADD, self_value, call_args, (size_t){argc}, depth, thrown); if (KILN_LIKELY(status == KILN_C99_OK)) goto kiln_method_done_{index}; }} }} "),
                "exec" => format!("if (KILN_LIKELY(self_value.tag == KILN_VALUE_REGEXP)) {{ status = kiln_c99_invoke_method(host, &call_result, KILN_C99_METHOD_REGEXP_EXEC, self_value, call_args, (size_t){argc}, depth, thrown); if (KILN_LIKELY(status == KILN_C99_OK)) goto kiln_method_done_{index}; }} "),
                _ => String::new(),
            };
            writeln!(
                output,
                "    {{ kiln_value call_result = kiln_fast_undefined(); kiln_value self_value = stack[sp - {argc} - 1]; int status; kiln_value call_argbuf[({argc}) + 1]; for (size_t i = 0; i < (size_t){argc}; i++) {{ call_argbuf[i] = stack[sp - {argc} + i]; }} const kiln_value *call_args = {argc} == 0 ? NULL : call_argbuf; {fast_method}static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; kiln_value method; status = kiln_c99_get_site(host, self_value, {symbol}_key_{name}, {}, &method, depth, thrown, &site); if (status == KILN_C99_CONTROL_ERROR) {{ if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = self_value.tag; if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = \"CallMethod property lookup refused\"; }} if (status == KILN_C99_CONTROL_ERROR) kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); if (status == KILN_C99_THROWN) {{ if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = \"CallMethod property lookup threw\"; kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); {unwind} }} if (status != KILN_C99_OK) return status; status = kiln_c99_call(host, &call_result, method, self_value, call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_CONTROL_ERROR && call_result.tag == KILN_VALUE_STRING) {{ *result = call_result; return status; }} if (status == KILN_C99_CONTROL_ERROR) {{ if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = self_value.tag; if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = \"CallMethod invocation refused\"; }} if (status == KILN_C99_CONTROL_ERROR) kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); if (status == KILN_C99_THROWN) {{ if (kiln_c99_diagnostic_detail == NULL) {{ kiln_c99_diagnostic_receiver_tag = self_value.tag; kiln_c99_diagnostic_detail = \"CallMethod invocation threw\"; }} kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); {unwind} }} if (status != KILN_C99_OK) return status; kiln_method_done_{index}:; sp -= (size_t){}; stack[sp++] = call_result; }}",
                text.len(),
                argc + 1
            )
            .unwrap();
        }
        Instruction::LoadBuiltin(kiln_ir::Builtin::GlobalThis) => {
            // The one builtin that is built rather than named: it is a real object, because a
            // program writes to it.
            output.push_str(
                "    { int status = kiln_c99_global_object(&stack[sp]); if (status != KILN_C99_OK) return status; ++sp; }\n",
            );
        }
        Instruction::LoadBuiltin(kiln_ir::Builtin::Process) => {
            output.push_str(
                "    { int status = kiln_c99_process_object(&stack[sp]); if (status != KILN_C99_OK) return status; ++sp; }\n",
            );
        }
        Instruction::LoadBuiltin(kiln_ir::Builtin::Atomics) => {
            output.push_str(
                "    { int status = kiln_c99_atomics_object(&stack[sp]); if (status != KILN_C99_OK) return status; ++sp; }\n",
            );
        }
        Instruction::LoadBuiltin(kiln_ir::Builtin::SetImmediate) => {
            output.push_str(
                "    stack[sp++] = kiln_c99_method(KILN_C99_METHOD_SET_IMMEDIATE);\n",
            );
        }
        Instruction::LoadBuiltin(kiln_ir::Builtin::ClearImmediate) => {
            output.push_str(
                "    stack[sp++] = kiln_c99_method(KILN_C99_METHOD_CLEAR_IMMEDIATE);\n",
            );
        }
        Instruction::LoadBuiltin(builtin) => {
            // `NaN` and `Infinity` are values rather than callables, but they reach the program
            // the same way -- as a name the compiler resolves to something the runtime supplies.
            let pushed = match builtin {
                kiln_ir::Builtin::String => "kiln_c99_builtin(KILN_C99_BUILTIN_STRING)",
                kiln_ir::Builtin::Math => "kiln_c99_builtin(KILN_C99_BUILTIN_MATH)",
                kiln_ir::Builtin::Number => "kiln_c99_builtin(KILN_C99_BUILTIN_NUMBER)",
                kiln_ir::Builtin::Object => "kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT)",
                kiln_ir::Builtin::Array => "kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY)",
                kiln_ir::Builtin::Json => "kiln_c99_builtin(KILN_C99_BUILTIN_JSON)",
                kiln_ir::Builtin::Date => "kiln_c99_builtin(KILN_C99_BUILTIN_DATE)",
                kiln_ir::Builtin::Map => "kiln_c99_builtin(KILN_C99_BUILTIN_MAP)",
                kiln_ir::Builtin::Set => "kiln_c99_builtin(KILN_C99_BUILTIN_SET)",
                kiln_ir::Builtin::WeakMap => "kiln_c99_builtin(KILN_C99_BUILTIN_WEAK_MAP)",
                kiln_ir::Builtin::WeakSet => "kiln_c99_builtin(KILN_C99_BUILTIN_WEAK_SET)",
                kiln_ir::Builtin::Promise => "kiln_c99_builtin(KILN_C99_BUILTIN_PROMISE)",
                kiln_ir::Builtin::Error => "kiln_c99_builtin(KILN_C99_BUILTIN_ERROR)",
                kiln_ir::Builtin::TypeError => "kiln_c99_builtin(KILN_C99_BUILTIN_TYPE_ERROR)",
                kiln_ir::Builtin::RangeError => "kiln_c99_builtin(KILN_C99_BUILTIN_RANGE_ERROR)",
                kiln_ir::Builtin::ReferenceError => {
                    "kiln_c99_builtin(KILN_C99_BUILTIN_REFERENCE_ERROR)"
                }
                kiln_ir::Builtin::SyntaxError => "kiln_c99_builtin(KILN_C99_BUILTIN_SYNTAX_ERROR)",
                kiln_ir::Builtin::UriError => "kiln_c99_builtin(KILN_C99_BUILTIN_URI_ERROR)",
                kiln_ir::Builtin::EvalError => "kiln_c99_builtin(KILN_C99_BUILTIN_EVAL_ERROR)",
                kiln_ir::Builtin::AggregateError => {
                    "kiln_c99_builtin(KILN_C99_BUILTIN_AGGREGATE_ERROR)"
                }
                kiln_ir::Builtin::Boolean => "kiln_c99_builtin(KILN_C99_BUILTIN_BOOLEAN)",
                // A global function is a namespace method with no namespace: nothing is reached
                // through to get at it, so the method value is what the name resolves to.
                kiln_ir::Builtin::ParseInt => "kiln_c99_method(KILN_C99_METHOD_PARSE_INT)",
                kiln_ir::Builtin::ParseFloat => "kiln_c99_method(KILN_C99_METHOD_PARSE_FLOAT)",
                kiln_ir::Builtin::IsNaN => "kiln_c99_method(KILN_C99_METHOD_GLOBAL_IS_NAN)",
                kiln_ir::Builtin::IsFinite => "kiln_c99_method(KILN_C99_METHOD_GLOBAL_IS_FINITE)",
                kiln_ir::Builtin::Reflect => "kiln_c99_builtin(KILN_C99_BUILTIN_REFLECT)",
                kiln_ir::Builtin::Proxy => "kiln_c99_builtin(KILN_C99_BUILTIN_PROXY)",
                kiln_ir::Builtin::Symbol => "kiln_c99_builtin(KILN_C99_BUILTIN_SYMBOL)",
                kiln_ir::Builtin::RegExp => "kiln_c99_builtin(KILN_C99_BUILTIN_REGEXP)",
                kiln_ir::Builtin::Function => "kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION)",
                kiln_ir::Builtin::BigInt => "kiln_c99_builtin(KILN_C99_BUILTIN_BIGINT)",
                kiln_ir::Builtin::SetTimeout => "kiln_c99_method(KILN_C99_METHOD_SET_TIMEOUT)",
                kiln_ir::Builtin::SetInterval => "kiln_c99_method(KILN_C99_METHOD_SET_INTERVAL)",
                kiln_ir::Builtin::ClearTimeout => {
                    "kiln_c99_method(KILN_C99_METHOD_CLEAR_TIMEOUT)"
                }
                kiln_ir::Builtin::ClearInterval => {
                    "kiln_c99_method(KILN_C99_METHOD_CLEAR_INTERVAL)"
                }
                kiln_ir::Builtin::QueueMicrotask => {
                    "kiln_c99_method(KILN_C99_METHOD_QUEUE_MICROTASK)"
                }
                kiln_ir::Builtin::Escape => "kiln_c99_method(KILN_C99_METHOD_ESCAPE)",
                kiln_ir::Builtin::Unescape => "kiln_c99_method(KILN_C99_METHOD_UNESCAPE)",
                kiln_ir::Builtin::EncodeUri => "kiln_c99_method(KILN_C99_METHOD_ENCODE_URI)",
                kiln_ir::Builtin::EncodeUriComponent => {
                    "kiln_c99_method(KILN_C99_METHOD_ENCODE_URI_COMPONENT)"
                }
                kiln_ir::Builtin::DecodeUri => "kiln_c99_method(KILN_C99_METHOD_DECODE_URI)",
                kiln_ir::Builtin::DecodeUriComponent => {
                    "kiln_c99_method(KILN_C99_METHOD_DECODE_URI_COMPONENT)"
                }
                kiln_ir::Builtin::ArrayBuffer => "kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER)",
                kiln_ir::Builtin::Int8Array => "kiln_c99_builtin(KILN_C99_BUILTIN_INT8_ARRAY)",
                kiln_ir::Builtin::Uint8Array => "kiln_c99_builtin(KILN_C99_BUILTIN_UINT8_ARRAY)",
                kiln_ir::Builtin::Uint8ClampedArray => {
                    "kiln_c99_builtin(KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY)"
                }
                kiln_ir::Builtin::Int16Array => "kiln_c99_builtin(KILN_C99_BUILTIN_INT16_ARRAY)",
                kiln_ir::Builtin::Uint16Array => "kiln_c99_builtin(KILN_C99_BUILTIN_UINT16_ARRAY)",
                kiln_ir::Builtin::Int32Array => "kiln_c99_builtin(KILN_C99_BUILTIN_INT32_ARRAY)",
                kiln_ir::Builtin::Uint32Array => "kiln_c99_builtin(KILN_C99_BUILTIN_UINT32_ARRAY)",
                kiln_ir::Builtin::Float32Array => "kiln_c99_builtin(KILN_C99_BUILTIN_FLOAT32_ARRAY)",
                kiln_ir::Builtin::Float64Array => "kiln_c99_builtin(KILN_C99_BUILTIN_FLOAT64_ARRAY)",
                kiln_ir::Builtin::Url => "kiln_c99_builtin(KILN_C99_BUILTIN_URL)",
                kiln_ir::Builtin::Buffer => "kiln_c99_builtin(KILN_C99_BUILTIN_BUFFER)",
                kiln_ir::Builtin::NotANumber => "kiln_c99_number(0.0 / 0.0)",
                kiln_ir::Builtin::Infinity => "kiln_c99_number(1.0 / 0.0)",
                kiln_ir::Builtin::AbortController => {
                    "kiln_c99_builtin(KILN_C99_BUILTIN_ABORT_CONTROLLER)"
                }
                kiln_ir::Builtin::AbortSignal => {
                    "kiln_c99_builtin(KILN_C99_BUILTIN_ABORT_SIGNAL)"
                }
                kiln_ir::Builtin::TextEncoder => {
                    "kiln_c99_builtin(KILN_C99_BUILTIN_TEXT_ENCODER)"
                }
                // Not built like `Atomics` and `process` are, which are real objects because a
                // program writes to them. Nothing writes to `performance`, so it stays the
                // built-in value it already was on `globalThis` -- and the two spellings reach
                // the same value without a slot to keep in step.
                kiln_ir::Builtin::Performance => {
                    "kiln_c99_builtin(KILN_C99_BUILTIN_PERFORMANCE)"
                }
                _ => unreachable!("verification rejects every other builtin before emission"),
            };
            writeln!(output, "    stack[sp++] = {pushed};").unwrap();
        }
        Instruction::Pop => output.push_str("    --sp;\n"),
        Instruction::Dup => output.push_str("    stack[sp] = stack[sp - 1]; ++sp;\n"),
        // A scalar program holds no reference to reduce, and has nowhere to put a thrown
        // value either, so its emitted bytes stay what they were.
        Instruction::Unary(operator) if !*callables => {
            writeln!(output, "    stack[sp - 1] = kiln_c99_unary({}, stack[sp - 1]);", unary_code(*operator)).unwrap()
        }
        Instruction::Unary(operator) => {
            if known_number_binaries.contains(&index)
                && let Some(fast) = numeric_fast_unary_body(*operator) {
                writeln!(output, "    {{ kiln_value operand = stack[sp - 1]; double a = kiln_fast_to_double(operand.bits); kiln_value answer; {fast} stack[sp - 1] = answer; }}").unwrap();
                return;
            }
            let unwind = unwind_or_return(handlers, site);
            match numeric_fast_unary_body(*operator) {
                Some(fast) => writeln!(output, "    {{ kiln_value operand = stack[sp - 1]; kiln_value answer; if (KILN_LIKELY(operand.tag == KILN_VALUE_NUMBER)) {{ double a = kiln_fast_to_double(operand.bits); {fast} }} else {{ int status = kiln_c99_unary_checked({}, host, operand, &answer, depth, thrown); if (KILN_UNLIKELY(status == KILN_C99_THROWN)) {unwind} if (KILN_UNLIKELY(status != KILN_C99_OK)) return status; }} stack[sp - 1] = answer; }}", unary_code(*operator)).unwrap(),
                None => writeln!(output, "    {{ kiln_value answer; int status = kiln_c99_unary_checked({}, host, stack[sp - 1], &answer, depth, thrown); if (KILN_UNLIKELY(status == KILN_C99_THROWN)) {unwind} if (KILN_UNLIKELY(status != KILN_C99_OK)) return status; stack[sp - 1] = answer; }}", unary_code(*operator)).unwrap(),
            }
        }
        Instruction::Binary(operator) if !*callables => {
            // A scalar program has no BigInt in it and so no operator in it can throw. It also
            // has nowhere to put a thrown value -- the entry point declares that slot only when
            // something can reach it -- so this is the form that program has to keep.
            match numeric_fast_body(*operator) {
                Some(fast) => writeln!(output, "    {{ kiln_value right = stack[--sp]; kiln_value left = stack[--sp]; kiln_value answer; if (KILN_LIKELY(left.tag == KILN_VALUE_NUMBER && right.tag == KILN_VALUE_NUMBER)) {{ double a = kiln_fast_to_double(left.bits); double b = kiln_fast_to_double(right.bits); {fast} }} else {{ answer = kiln_c99_binary({}, left, right); }} stack[sp++] = answer; }}", binary_code(*operator)).unwrap(),
                None => writeln!(output, "    {{ kiln_value right = stack[--sp]; kiln_value left = stack[--sp]; stack[sp++] = kiln_c99_binary({}, left, right); }}", binary_code(*operator)).unwrap(),
            }
        }
        Instruction::Binary(operator) => {
            if known_number_binaries.contains(&index)
                && let Some(fast) = numeric_fast_body(*operator) {
                writeln!(output, "    {{ kiln_value right = stack[--sp]; kiln_value left = stack[--sp]; double a = kiln_fast_to_double(left.bits); double b = kiln_fast_to_double(right.bits); kiln_value answer; {fast} stack[sp++] = answer; }}").unwrap();
                return;
            }
            // An operator throws only when a BigInt is involved, and `kiln_c99_binary_checked`
            // hands every other case straight back to `kiln_c99_binary`.
            let unwind = unwind_or_return(handlers, site);
            match numeric_fast_body(*operator) {
                Some(fast) => writeln!(output, "    {{ kiln_value right = stack[--sp]; kiln_value left = stack[--sp]; kiln_value answer; if (KILN_LIKELY(left.tag == KILN_VALUE_NUMBER && right.tag == KILN_VALUE_NUMBER)) {{ double a = kiln_fast_to_double(left.bits); double b = kiln_fast_to_double(right.bits); {fast} }} else {{ int status = kiln_c99_binary_checked({}, host, left, right, &answer, depth, thrown); if (KILN_UNLIKELY(status == KILN_C99_THROWN)) {unwind} if (KILN_UNLIKELY(status != KILN_C99_OK)) return status; }} stack[sp++] = answer; }}", binary_code(*operator)).unwrap(),
                None => writeln!(output, "    {{ kiln_value right = stack[--sp]; kiln_value left = stack[--sp]; kiln_value answer; int status = kiln_c99_binary_checked({}, host, left, right, &answer, depth, thrown); if (KILN_UNLIKELY(status == KILN_C99_THROWN)) {unwind} if (KILN_UNLIKELY(status != KILN_C99_OK)) return status; stack[sp++] = answer; }}", binary_code(*operator)).unwrap(),
            }
        }
        Instruction::Jump(target) => writeln!(output, "    goto kiln_label_{target};").unwrap(),
        Instruction::JumpIfFalse(target) => {
            writeln!(output, "    if (!kiln_fast_truthy(stack[--sp])) goto kiln_label_{target};").unwrap()
        }
        Instruction::JumpIfTruthy(target) => {
            writeln!(output, "    if (kiln_fast_truthy(stack[--sp])) goto kiln_label_{target};").unwrap()
        }
        Instruction::JumpIfNotNullish(target) => {
            writeln!(output, "    {{ kiln_value value = stack[--sp]; if (value.tag != KILN_VALUE_UNDEFINED && value.tag != KILN_VALUE_NULL) goto kiln_label_{target}; }}").unwrap()
        }
        Instruction::HostCall { op, argc } => {
            // The embedding boundary carries only the five scalar tags, so a function argument
            // is reduced to its text before the host sees it -- which is also what the
            // interpreter prints for one.
            if *argc > 0 && *callables {
                writeln!(output, "    {{ size_t arg; for (arg = sp - {argc}; arg < sp; ++arg) if (kiln_c99_crosses_boundary(stack[arg])) stack[arg] = kiln_c99_inspect(stack[arg]); }}").unwrap();
            }
            // The arguments are copied into a fresh buffer rather than passed as `&stack[sp - n]`.
            // Pointing into `stack[]` and handing that pointer to the host makes the stack array
            // address-escaped for the entire function, which defeats the promotion the numeric
            // fast paths exist to enable: a single `console.log` at the end of a program was
            // enough to keep a fifty-million-iteration loop's values in memory. The buffer is a
            // local whose address escaping costs nothing, because nothing else reads it.
            if *argc == 0 {
                writeln!(output, "    {{ kiln_value host_result; if (host == NULL || host->call == NULL || host->call(host->context, UINT16_C({}), UINT16_C(0), NULL, &host_result) != 0) return KILN_C99_HOST_ERROR; stack[sp++] = host_result; }}", host_op_code(*op)).unwrap()
            } else {
                writeln!(output, "    {{ kiln_value host_result; kiln_value host_args[{argc}]; for (size_t i = 0; i < (size_t){argc}; i++) {{ host_args[i] = stack[sp - {argc} + i]; }} if (host == NULL || host->call == NULL || host->call(host->context, UINT16_C({}), UINT16_C({argc}), host_args, &host_result) != 0) return KILN_C99_HOST_ERROR; sp -= {argc}; stack[sp++] = host_result; }}", host_op_code(*op)).unwrap()
            }
        }
        Instruction::Return => match site {
            ReturnSite::Body => output.push_str(
                "    *result = sp == 0 ? kiln_c99_undefined() : stack[--sp]; return KILN_C99_OK;\n",
            ),
            ReturnSite::Entry {
                exports: None, ..
            } if !callables => output.push_str(
                "    *result = sp == 0 ? kiln_c99_undefined() : stack[--sp]; return KILN_C99_OK;\n",
            ),
            ReturnSite::Entry {
                exports: None, ..
            } => output.push_str(
                "    *result = sp == 0 ? kiln_c99_undefined() : stack[--sp];\n    if (kiln_c99_crosses_boundary(*result)) *result = kiln_c99_text(*result);\n    return KILN_C99_OK;\n",
            ),
            ReturnSite::Entry {
                exports: Some(exports),
                commonjs,
            } => {
                output.push_str(
                    "    *result = sp == 0 ? kiln_c99_undefined() : stack[--sp];\n",
                );
                if *callables {
                    output.push_str(
                        "    if (kiln_c99_crosses_boundary(*result)) *result = kiln_c99_text(*result);\n",
                    );
                }
                if *commonjs {
                    // `depth` and `thrown` are declared by the entry point only when the module
                    // has callables in it, so a CommonJS module that declares no function at all
                    // -- `console.log("ok")` in a `.cjs` is the whole of one -- named two
                    // identifiers that were not there, and the C would not compile. It is the
                    // read that has to bend rather than the declaration: with no callable
                    // anywhere in the module there is no getter that could have been installed on
                    // `module`, so this read provably cannot re-enter or throw and the depth
                    // counter and the thrown slot have nothing to carry.
                    let (depth, thrown) = if *callables {
                        ("depth", "thrown")
                    } else {
                        ("0", "NULL")
                    };
                    writeln!(
                        output,
                        "    {{ kiln_value module_value = locals[2].tag == KILN_VALUE_CELL ? *kiln_c99_cell_of(locals[2]) : locals[2]; int export_status = kiln_c99_get(host, module_value, \"exports\", 7, &exports[0], {depth}, {thrown}); if (export_status != KILN_C99_OK) return export_status; }}"
                    )
                    .unwrap();
                }
                for (index, binding) in exports.iter().enumerate() {
                    // The slot is handed over exactly as it stands, box included. A mutable
                    // export *is* its cell: that is what makes an import a live view of the
                    // exporter's binding rather than a copy of the value it happened to hold
                    // when the exporter finished. Only an export that can change is boxed, so
                    // an immutable one arrives here as a plain value and stays one.
                    writeln!(
                        output,
                        "    exports[{index}] = locals[{}];",
                        binding.local_slot
                    )
                    .unwrap();
                }
                output.push_str("    return KILN_C99_OK;\n");
            }
        },
    }
    {
        let emitted = &mut output[before..];
        /* Every generated body owns a refusal label and its result slot. Routing through it is
         * what materializes the diagnostic while this translation unit's private state is still
         * visible. Returning directly from a closure loses that state when an imported function
         * crosses back to its caller. */
        let refusal_target = "goto kiln_refusal;";
        let refusal = format!(
            "{control_refusal} {refusal_target} }} if (status != KILN_C99_OK) return status;"
        );
        let replaced = emitted.replace("if (status != KILN_C99_OK) return status;", &refusal);
        output.truncate(before);
        output.push_str(&replaced);
    }
}

fn instruction_diagnostic(instruction: &Instruction, constants: &[Constant]) -> String {
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
        operation.push_str(name);
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

/// The C statement that answers a binary operator when both operands are already numbers.
///
/// The operator is known here, at emission time. The first version of this fast path passed it to
/// a shared runtime helper as a `uint16_t` and let that helper switch on it — which took static
/// information and made it dynamic, the same mistake ADR 0016 diagnoses in the generic dispatcher,
/// one layer down. The disassembly was unambiguous: `mov w0, #0x9` followed by `bl
/// kiln_c99_binary_fast`, with both operands passed by address and the result read back through
/// memory, because a function holding a 22-way switch and a fallback call is not inlinable however
/// it is annotated.
///
/// Emitting the operation itself removes three costs at once: the call, the switch, and — the one
/// that matters most — the opacity. With no out-of-line call left in a numeric loop body, nothing
/// forces `stack[]` and `locals[]` to memory, so SROA can promote them. That is the promotion
/// ADR 0016 predicted would happen "if the operations were visible"; this is what makes them
/// visible.
///
/// `None` means there is no cheap numeric answer and the site keeps the generic call: `**` needs
/// `pow`, and the bitwise operators and shifts need ToInt32, which is a separate decision.
fn numeric_fast_body(operator: BinaryOp) -> Option<&'static str> {
    Some(match operator {
        BinaryOp::Add => "answer = kiln_fast_number(a + b);",
        BinaryOp::Subtract => "answer = kiln_fast_number(a - b);",
        BinaryOp::Multiply => "answer = kiln_fast_number(a * b);",
        BinaryOp::Divide => "answer = kiln_fast_number(a / b);",
        // `%` is `fmod` by specification, and `fmod` is a call into libm that measured at 41% of a
        // Collatz benchmark's entire runtime. Two integral operands take the machine instruction
        // instead, which agrees exactly: C truncates toward zero and takes the sign of the
        // dividend, as ECMAScript specifies. The guard stays conservative — outside +/-2^53 a
        // double is not exactly an integer, a zero divisor must reach `fmod` for NaN, and a zero
        // dividend must too, because `-0 % 3` is `-0` and integer division would answer `+0`.
        BinaryOp::Remainder => {
            "if (a != 0.0 && b != 0.0 && a >= -9007199254740992.0 && a <= 9007199254740992.0 &&              b >= -9007199254740992.0 && b <= 9007199254740992.0 && a == (double)(int64_t)a &&              b == (double)(int64_t)b) { answer = kiln_fast_number((double)((int64_t)a % (int64_t)b)); }              else { answer = kiln_fast_number(fmod(a, b)); }"
        }
        BinaryOp::StrictEqual => "answer = kiln_fast_boolean(a == b);",
        BinaryOp::StrictNotEqual => "answer = kiln_fast_boolean(a != b);",
        BinaryOp::LessThan => "answer = kiln_fast_boolean(a < b);",
        BinaryOp::LessThanOrEqual => "answer = kiln_fast_boolean(a <= b);",
        BinaryOp::GreaterThan => "answer = kiln_fast_boolean(a > b);",
        BinaryOp::GreaterThanOrEqual => "answer = kiln_fast_boolean(a >= b);",
        _ => return None,
    })
}

/// The unary counterpart of [`numeric_fast_body`], answering in terms of `a`.
///
/// `typeof` and `void` do not touch the value and `~` needs ToInt32, so they keep the generic call.
fn numeric_fast_unary_body(operator: UnaryOp) -> Option<&'static str> {
    Some(match operator {
        UnaryOp::Plus | UnaryOp::ToNumeric => "answer = kiln_fast_number(a);",
        UnaryOp::Negate => "answer = kiln_fast_number(-a);",
        // `!x` on a number is false unless the number is zero or NaN.
        UnaryOp::Not => "answer = kiln_fast_boolean(a == 0.0 || a != a);",
        UnaryOp::Increment => "answer = kiln_fast_number(a + 1.0);",
        UnaryOp::Decrement => "answer = kiln_fast_number(a - 1.0);",
        _ => return None,
    })
}

/// Whether a call into this function must publish the per-frame context.
///
/// `kiln_c99_invoke_closure_body` saves and restores `new_target`, `frame_args`, `frame_argc`,
/// `captured_arguments` and the arrow flag around every call. Only three instructions can read
/// them -- `LoadArguments`, `LoadNewTarget`, `RestArguments` -- so a function containing none of
/// them cannot tell whether the publication happened. Skipping it measured 1.48x on a recursive
/// `fib`, which is call-bound and reads neither.
///
/// Conservative in four directions beyond that, because a wrong answer here is a silently wrong
/// `arguments` rather than a slow program:
///
///   - A function that creates any closure needs it: an arrow captures the enclosing frame's
///     `arguments` at `MakeFunction` time, so the context must be live then even though this
///     function never reads it.
///   - An arrow needs it, because its own `arguments` resolves to the captured value.
///   - A generator or async function needs it, because its body is entered and re-entered
///     through paths that do not all pass through this predicate.
///
/// The diagnostic depth and refusal attribution are performed either way, so provenance is
/// unaffected: this decides only the fields the language can observe.
fn proto_publishes_frame(proto: &FunctionProto) -> bool {
    proto.arrow
        || proto.generator
        || proto.asynchronous
        || proto.async_generator
        || proto.code.iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::LoadArguments
                    | Instruction::LoadNewTarget
                    | Instruction::RestArguments(_)
                    | Instruction::MakeFunction(_)
            )
        })
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
                for value in value.bytes() {
                    byte(&mut hash, value);
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
            Instruction::HostCall { op, argc } => {
                byte(&mut hash, 11);
                word(&mut hash, u64::from(host_op_code(*op)));
                word(&mut hash, u64::from(*argc));
            }
            Instruction::Return => byte(&mut hash, 12),
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
    match &function.name {
        Some(name) => {
            byte(hash, 1);
            word(hash, name.len() as u64);
            for value in name.bytes() {
                byte(hash, value);
            }
        }
        None => byte(hash, 0),
    }
    byte(hash, u8::from(function.arrow));
    byte(hash, u8::from(function.generator));
    // Appended after `generator`, which is where a new flag has to go: the bytes before it are a
    // persisted cache key and cannot move.
    byte(hash, u8::from(function.async_generator));
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
            Upvalue::ParentLocal(index) => {
                byte(hash, 0);
                word(hash, u64::from(*index));
            }
            Upvalue::ParentUpvalue(index) => {
                byte(hash, 1);
                word(hash, u64::from(*index));
            }
        }
    }
}

fn hash_module_pair(module: &ModuleProgram) -> (u64, u64) {
    hash_module_pair_with_policy(module, &EmissionPolicy::default())
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
