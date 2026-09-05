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
pub const C_GENERATOR_VERSION: u32 = 29;

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
    let source = render(program, program_hash, &symbol, &policy)?;
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
    let source = render_module(module, program_hash, &symbol, &policy, false, &symbol)?;
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
    let source = render_module(module, program_hash, &symbol, &policy, false, logical_path)?;
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
    let source = render_module(module, program_hash, &symbol, &policy, true, logical_path)?;
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

pub mod emit_graph;
pub mod facts;
pub mod graph;
pub mod summary;
mod tail;
pub mod verifier;
pub use emit_graph::emit_function_from_graph;
pub use facts::{
    FunctionFacts, ModuleFactsReport, ProgramFacts, SiteFact, analyze_graph, analyze_program,
};
pub use graph::{
    BasicBlock, BlockId, FunctionGraph, GraphOp, SiteId, Terminator, ValueId, build_function_graph,
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
    reaches(&program.code) || program.functions.iter().any(|proto| reaches(&proto.code))
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
    let (name, name_len) = match &target.name {
        Some(text) => (format!("{symbol}_name_{proto}"), text.len()),
        None => ("NULL".to_owned(), 0),
    };
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
    let diagnostic_source = c_string(diagnostic_site);
    format!(
        "kiln_c99_make_closure(&built, {symbol}_fn_{proto}, {name}, {name_len}, {source}, {source_len}, {diagnostic_source}, UINT32_C({proto}), UINT32_C({arity}), {generator}, {asynchronous}, {async_generator}, {arrow}, {strict}, {defining_receiver}, kiln_val_from_boundary(kiln_c99_new_target), UINT32_C({frame_locals}), UINT32_C({frame_stack}), UINT32_C({frame_handlers}), UINT32_C({prologue_resume}), {captured}, UINT32_C({capture_count}))"
    )
}

fn render_functions(
    output: &mut String,
    program: &Program,
    plan: &StackPlan,
    symbol: &str,
    diagnostic_site: &str,
    strict_functions: &[bool],
) -> Result<(), EmitError> {
    for index in 0..program.functions.len() {
        writeln!(
            output,
            "static inline int {symbol}_fn_{index}(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues, const kiln_val *receiver_ptr, const kiln_val *args, size_t argc);"
        )
        .unwrap();
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
        )?;
    }
    Ok(())
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
) -> Result<(), EmitError> {
    let scalar_proto = !proto_produces_references(proto, program);
    let graph = graph_for(
        proto,
        program,
        &format!("function {index}"),
        diagnostic_site,
    )?;
    emit_function_from_graph(
        output,
        &graph,
        proto,
        program,
        symbol,
        index,
        scalar_proto,
        diagnostic_site,
        frame_stacks,
        strict_functions,
    );
    Ok(())
}

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
        asynchronous: false,
        name: None,
        arrow: false,
        generator: false,
        async_generator: false,
        parameter_count: 0,
        mapped_arguments: false,
        declared_length: 0,
        local_count: program.local_count,
        prologue_end: 0,
        code: program.code.clone(),
        upvalues: Vec::new(),
        span: kiln_ir::Span::default(),
    }
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
    symbol: &str,
    diagnostic_site: &str,
    strict_functions: &[bool],
    keyed: bool,
    module: Option<(&ModuleProgram, bool)>,
) -> Result<(), EmitError> {
    let callables = produces_references(program);
    let entry_symbol = if callables {
        format!("{symbol}_body")
    } else {
        symbol.to_owned()
    };
    let module_scope = uses_module_scope(program);
    let body = entry_proto(program);
    let graph = graph_for(&body, program, "the module body", diagnostic_site)?;
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
        "    (void)kiln_realm_gc_register_roots({symbol}_gc_roots, NULL, 0);"
    )
    .unwrap();
    render_key_prologue(output, symbol, keyed);
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
        writeln!(output, "    kiln_val *locals = {symbol}_globals;").unwrap();
    } else {
        writeln!(output, "    kiln_val locals[{locals}];").unwrap();
    }
    output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
    let mut array_slots: HashSet<u32> = if module_scope {
        (0..program.local_count).collect()
    } else {
        emit_graph::array_slots(&graph, program)
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
    emit_graph::emit_entry_from_graph(
        output,
        &graph,
        &body,
        program,
        symbol,
        !callables,
        diagnostic_site,
        &plan.functions,
        strict_functions,
        &array_slots,
        exports,
        commonjs,
    );
    output.push_str("}\n");
    Ok(())
}

/// The constant table.
///
/// A string is its bytes. A BigInt is its digits plus a slot for the handle the provider builds
/// from them on first use: parsing is not free and a literal's value never changes, so the unit
/// pays for it once however often the literal is loaded.
fn render_constant_declarations(output: &mut String, program: &Program, symbol: &str) {
    for (index, constant) in program.constants.iter().enumerate() {
        match constant {
            Constant::String(value) => {
                writeln!(
                    output,
                    "static const char {symbol}_constant_{index}[] = {};",
                    c_string(value)
                )
                .unwrap();
                if value.contains('\0') {
                    writeln!(output, "static void *{symbol}_string_{index} = NULL;").unwrap();
                }
            }
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

fn render(
    program: &Program,
    program_hash: u64,
    symbol: &str,
    policy: &EmissionPolicy,
) -> Result<String, EmitError> {
    let plan = validate_program(program).expect("render called after validation");
    // A bare `Program` arrives with no goal and no strictness table: it is a scalar unit, and
    // every one this backend is handed came from a Module goal. Strict throughout keeps its
    // emitted bytes exactly what they were.
    let strict_functions = vec![true; program.functions.len()];
    let callables = produces_references(program);
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
    let keys = property_key_constants(program);
    render_key_table(&mut output, program, symbol, &keys);
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
    );
    render_functions(
        &mut output,
        program,
        &plan,
        symbol,
        symbol,
        &strict_functions,
    )?;
    render_entry(
        &mut output,
        program,
        &plan,
        symbol,
        symbol,
        &strict_functions,
        !keys.is_empty(),
        None,
    )?;
    if callables {
        render_scalar_realm_wrapper(&mut output, symbol);
    }
    Ok(output)
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
    let keys = property_key_constants(program);
    render_key_table(&mut output, program, symbol, &keys);
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
    );
    render_functions(
        &mut output,
        program,
        &plan,
        symbol,
        diagnostic_site,
        &strict_functions,
    )?;
    render_entry(
        &mut output,
        program,
        &plan,
        symbol,
        diagnostic_site,
        &strict_functions,
        !keys.is_empty(),
        Some((module, commonjs)),
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
        render_module_realm_wrapper(&mut output, symbol);
    }
    Ok(output)
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
            Instruction::DefineNamedProperty => byte(&mut hash, 194),
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
