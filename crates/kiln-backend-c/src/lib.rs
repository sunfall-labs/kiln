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
macro_rules! provider_http_close_c_declaration {
    ($(($group:ident, $name:ident, $signature:ident);)*) => {
        concat!($(provider_http_close_c_declaration!($group, $name, $signature)),*)
    };
    (realm, kiln_realm_http_server_close_active, realm_http_close_mode) => {
        "int kiln_realm_http_server_close_active(uint64_t resource, int mode);"
    };
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

#[derive(Clone, Copy)]
struct MethodId {
    name: &'static str,
    value: u16,
    groups: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NativePrototypeSurface {
    EventEmitter,
    Readable,
    Writable,
    ServerResponse,
}

#[derive(Clone, Copy)]
struct NativeConstructor {
    method: u16,
    parent: Option<u16>,
    surface: NativePrototypeSurface,
}

const fn validate_native_constructors(constructors: &[NativeConstructor], methods: &[MethodId]) {
    let mut index = 0;
    while index < constructors.len() {
        let constructor = constructors[index];
        let mut method_found = false;
        let mut method_index = 0;
        while method_index < methods.len() {
            if methods[method_index].value == constructor.method {
                method_found = true;
                break;
            }
            method_index += 1;
        }
        assert!(
            method_found,
            "native constructor is not a registered method"
        );

        let mut other = index + 1;
        while other < constructors.len() {
            assert!(
                constructors[other].method != constructor.method,
                "duplicate native constructor"
            );
            other += 1;
        }

        if let Some(parent) = constructor.parent {
            let mut parent_index = 0;
            let mut parent_found = false;
            while parent_index < index {
                if constructors[parent_index].method == parent {
                    parent_found = true;
                    break;
                }
                parent_index += 1;
            }
            assert!(
                parent_found,
                "native constructor parent must be declared before its child"
            );
        }
        index += 1;
    }
}

fn render_native_constructor_table() -> String {
    let mut table =
        String::from("static kiln_native_constructor kiln_c99_native_constructors[] = {\n");
    for constructor in NATIVE_CONSTRUCTORS {
        let parent = constructor.parent.unwrap_or(0);
        let surface = match constructor.surface {
            NativePrototypeSurface::EventEmitter => "KILN_C99_NATIVE_EVENT_EMITTER",
            NativePrototypeSurface::Readable => "KILN_C99_NATIVE_READABLE",
            NativePrototypeSurface::Writable => "KILN_C99_NATIVE_WRITABLE",
            NativePrototypeSurface::ServerResponse => "KILN_C99_NATIVE_SERVER_RESPONSE",
        };
        use std::fmt::Write as _;
        writeln!(
            table,
            "    {{ UINT64_C({}), UINT64_C({parent}), {surface}, {{KILN_VALUE_UNDEFINED, 0, {{0}}}} }},",
            constructor.method
        )
        .expect("writing to String cannot fail");
    }
    table.push_str("};\n");
    table
}

macro_rules! native_constructor {
    ($method:ident, $parent:expr, $surface:ident) => {
        NativeConstructor {
            method: method_value!($method),
            parent: $parent,
            surface: NativePrototypeSurface::$surface,
        }
    };
}

macro_rules! method_group_bit {
    (Direct) => {
        1u32 << 0
    };
    (ArrayLookup) => {
        1u32 << 1
    };
    (StringDispatch) => {
        1u32 << 2
    };
    (Generic) => {
        1u32 << 3
    };
    (Temporal) => {
        1u32 << 4
    };
    (TemporalFrom) => {
        1u32 << 5
    };
    (TemporalCompare) => {
        1u32 << 6
    };
    (TemporalNowRead) => {
        1u32 << 7
    };
    (TemporalOp) => {
        1u32 << 8
    };
    (DateSetter) => {
        1u32 << 9
    };
    (NodeEvent) => {
        1u32 << 10
    };
    (NodeNet) => {
        1u32 << 11
    };
    (NodeServer) => {
        1u32 << 12
    };
    (NodeHttp) => {
        1u32 << 13
    };
    (NodeResponse) => {
        1u32 << 14
    };
    (Buffer) => {
        1u32 << 15
    };
    (NodeUtil) => {
        1u32 << 16
    };
    (NodeDiagnostics) => {
        1u32 << 17
    };
    (NodeStream) => {
        1u32 << 18
    };
    (NodePath) => {
        1u32 << 19
    };
    (MathNumber) => {
        1u32 << 20
    };
}

macro_rules! method_group_bits {
    ($first:ident $(, $rest:ident)*) => {
        method_group_bit!($first) $(| method_group_bit!($rest))*
    };
}

macro_rules! method_group_c_bit {
    (Direct) => {
        "UINT32_C(1)"
    };
    (ArrayLookup) => {
        "UINT32_C(2)"
    };
    (StringDispatch) => {
        "UINT32_C(4)"
    };
    (Generic) => {
        "UINT32_C(8)"
    };
    (Temporal) => {
        "UINT32_C(16)"
    };
    (TemporalFrom) => {
        "UINT32_C(32)"
    };
    (TemporalCompare) => {
        "UINT32_C(64)"
    };
    (TemporalNowRead) => {
        "UINT32_C(128)"
    };
    (TemporalOp) => {
        "UINT32_C(256)"
    };
    (DateSetter) => {
        "UINT32_C(512)"
    };
    (NodeEvent) => {
        "UINT32_C(1024)"
    };
    (NodeNet) => {
        "UINT32_C(2048)"
    };
    (NodeServer) => {
        "UINT32_C(4096)"
    };
    (NodeHttp) => {
        "UINT32_C(8192)"
    };
    (NodeResponse) => {
        "UINT32_C(16384)"
    };
    (Buffer) => {
        "UINT32_C(32768)"
    };
    (NodeUtil) => {
        "UINT32_C(65536)"
    };
    (NodeDiagnostics) => {
        "UINT32_C(131072)"
    };
    (NodeStream) => {
        "UINT32_C(262144)"
    };
    (NodePath) => {
        "UINT32_C(524288)"
    };
    (MathNumber) => {
        "UINT32_C(1048576)"
    };
}

macro_rules! method_group_c_bits {
    ($first:ident $(, $rest:ident)*) => {
        concat!(method_group_c_bit!($first) $(, " | ", method_group_c_bit!($rest))* )
    };
}

macro_rules! method_group_c_case {
    (KILN_C99_METHOD_TEMPORAL_LAST, $($group:ident),*) => { "" };
    ($name:ident, $first:ident $(, $group:ident)*) => {
        concat!(
            "    case ", stringify!($name), ": return ",
            method_group_c_bits!($first $(, $group)*), ";\n"
        )
    };
}

macro_rules! method_ids {
    ($(($name:ident, $value:literal, $first_group:ident $(, $group:ident)*)),* $(,)?) => {
        macro_rules! emit_method_define {
            $(($name) => {
                concat!("#define ", stringify!($name), " ", stringify!($value), "\n")
            };)*
        }

        macro_rules! method_value {
            $(($name) => { $value };)*
        }

        const METHOD_IDS: &[MethodId] = &[
            $(MethodId {
                name: stringify!($name),
                value: $value,
                groups: method_group_bits!($first_group $(, $group)*),
            }),*
        ];

        macro_rules! emit_method_group_cases {
            () => {
                concat!($(method_group_c_case!($name, $first_group $(, $group)*)),*)
            };
        }

        macro_rules! emit_method_code_list {
            () => { concat!($(stringify!($name), ", "),*) };
        }
    };
}

const fn method_name_is(name: &str, expected: &str) -> bool {
    let name = name.as_bytes();
    let expected = expected.as_bytes();
    if name.len() != expected.len() {
        return false;
    }
    let mut index = 0;
    while index < name.len() {
        if name[index] != expected[index] {
            return false;
        }
        index += 1;
    }
    true
}

const fn method_name_starts_with(name: &str, prefix: &str) -> bool {
    let name = name.as_bytes();
    let prefix = prefix.as_bytes();
    if name.len() < prefix.len() {
        return false;
    }
    let mut index = 0;
    while index < prefix.len() {
        if name[index] != prefix[index] {
            return false;
        }
        index += 1;
    }
    true
}

const fn method_name_is_temporal_last(name: &str) -> bool {
    method_name_is(name, "KILN_C99_METHOD_TEMPORAL_LAST")
}

const fn method_value(ids: &[MethodId], name: &str) -> u16 {
    let mut index = 0;
    while index < ids.len() {
        if method_name_is(ids[index].name, name) {
            return ids[index].value;
        }
        index += 1;
    }
    panic!("method ID is missing from the registry");
}

const fn assert_method_immediately_after(ids: &[MethodId], previous: &str, next: &str) {
    let previous = method_value(ids, previous);
    assert!(
        previous < u16::MAX && previous + 1 == method_value(ids, next),
        "Temporal method blocks must be ordered and adjacent"
    );
}

const fn validate_method_ids(ids: &[MethodId]) {
    // `kiln_c99_invoke_method` returns immediately from each of these adapter handlers. A method
    // in two such groups is therefore routed by source order rather than receiver shape, which is
    // never a meaningful contract. Receiver-specific secondary routing (for example raw-socket
    // `setEncoding`) must remain an explicit check in the generated dispatcher.
    let terminal_adapter_groups = method_group_bit!(NodeEvent)
        | method_group_bit!(NodeNet)
        | method_group_bit!(NodeHttp)
        | method_group_bit!(NodeResponse)
        | method_group_bit!(Buffer)
        | method_group_bit!(NodeUtil)
        | method_group_bit!(NodeDiagnostics)
        | method_group_bit!(NodeStream)
        | method_group_bit!(NodePath);
    let mut index = 0;
    while index < ids.len() {
        assert!(ids[index].value > 0, "method IDs must be positive");
        assert!(
            ids[index].groups != 0,
            "every method must have a dispatch category"
        );
        let terminal_groups = ids[index].groups & terminal_adapter_groups;
        let receiver_routed_set_encoding = method_name_is(
            ids[index].name,
            "KILN_C99_METHOD_NODE_READABLE_SET_ENCODING",
        ) && terminal_groups
            == method_group_bit!(NodeNet)
                | method_group_bit!(NodeHttp)
                | method_group_bit!(NodeStream);
        assert!(
            terminal_groups == 0
                || terminal_groups & (terminal_groups - 1) == 0
                || receiver_routed_set_encoding,
            "only explicitly receiver-routed setEncoding may span terminal adapter groups"
        );
        let mut other = index + 1;
        while other < ids.len() {
            assert!(
                !method_name_is(ids[index].name, ids[other].name),
                "method names must be unique"
            );
            if ids[index].value == ids[other].value {
                let alias = if method_name_is_temporal_last(ids[index].name) {
                    method_name_is(
                        ids[other].name,
                        "KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS",
                    )
                } else if method_name_is_temporal_last(ids[other].name) {
                    method_name_is(
                        ids[index].name,
                        "KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS",
                    )
                } else {
                    false
                };
                assert!(
                    alias
                        && method_name_starts_with(ids[index].name, "KILN_C99_METHOD_TEMPORAL_")
                        && method_name_starts_with(ids[other].name, "KILN_C99_METHOD_TEMPORAL_"),
                    "method IDs must be unique except for the Instant nanoseconds alias"
                );
            }
            other += 1;
        }
        index += 1;
    }

    // These are range-dispatched, so their endpoint constants must retain the width used by the
    // generated call path. The individual operation IDs between those endpoints are intentionally
    // not declared: the runtime derives their kind from the code itself.
    assert!(
        method_value(ids, "KILN_C99_METHOD_TEMPORAL_FROM_LAST")
            - method_value(ids, "KILN_C99_METHOD_TEMPORAL_FROM")
            == 7,
        "Temporal.from range must contain eight kinds"
    );
    assert!(
        method_value(ids, "KILN_C99_METHOD_TEMPORAL_COMPARE_LAST")
            - method_value(ids, "KILN_C99_METHOD_TEMPORAL_COMPARE")
            == 7,
        "Temporal.compare range must contain eight kinds"
    );
    assert!(
        method_value(ids, "KILN_C99_METHOD_TEMPORAL_NOW_READ_LAST")
            - method_value(ids, "KILN_C99_METHOD_TEMPORAL_NOW_READ")
            == 7,
        "Temporal.Now range must contain eight kinds"
    );
    assert!(
        method_value(ids, "KILN_C99_METHOD_TEMPORAL_OP_LAST")
            - method_value(ids, "KILN_C99_METHOD_TEMPORAL_OP")
            == 22,
        "Temporal operation range must contain 23 operations"
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_FROM_LAST",
        "KILN_C99_METHOD_TEMPORAL_COMPARE",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_COMPARE_LAST",
        "KILN_C99_METHOD_TEMPORAL_TO_STRING",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_TO_STRING",
        "KILN_C99_METHOD_TEMPORAL_TO_JSON",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_TO_JSON",
        "KILN_C99_METHOD_TEMPORAL_VALUE_OF",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_VALUE_OF",
        "KILN_C99_METHOD_TEMPORAL_NOW_TIME_ZONE_ID",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_NOW_TIME_ZONE_ID",
        "KILN_C99_METHOD_TEMPORAL_NOW_READ",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_NOW_READ_LAST",
        "KILN_C99_METHOD_TEMPORAL_OP",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_OP_LAST",
        "KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_MILLISECONDS",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_MILLISECONDS",
        "KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS",
    );

    let mut temporal_low = u16::MAX;
    let mut temporal_high = 0;
    let mut index = 0;
    while index < ids.len() {
        if method_name_starts_with(ids[index].name, "KILN_C99_METHOD_TEMPORAL_")
            && !method_name_is_temporal_last(ids[index].name)
        {
            if ids[index].value < temporal_low {
                temporal_low = ids[index].value;
            }
            if ids[index].value > temporal_high {
                temporal_high = ids[index].value;
            }
        }
        index += 1;
    }
    assert!(temporal_low < temporal_high, "Temporal methods must exist");
    assert!(
        temporal_low == method_value(ids, "KILN_C99_METHOD_TEMPORAL_FROM")
            && temporal_high
                == method_value(
                    ids,
                    "KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS",
                ),
        "Temporal range must begin at FROM and end at the final Instant constructor"
    );
    assert!(
        method_value(ids, "KILN_C99_METHOD_TEMPORAL_LAST") == temporal_high,
        "TEMPORAL_LAST must alias the highest Temporal method"
    );
    index = 0;
    while index < ids.len() {
        if !method_name_starts_with(ids[index].name, "KILN_C99_METHOD_TEMPORAL_") {
            assert!(
                ids[index].value < temporal_low || ids[index].value > temporal_high,
                "non-Temporal method lies inside a Temporal dispatch range"
            );
        }
        index += 1;
    }

    // IDs at and above the first Temporal.Instant constructor are append-only. This catches a
    // merge that shifts the equality-dispatched tail into a hole or reuses an old value.
    const APPEND_ONLY_BASE: u16 = 336;
    let mut tail_high = 0;
    index = 0;
    while index < ids.len() {
        if ids[index].value >= APPEND_ONLY_BASE && ids[index].value > tail_high {
            tail_high = ids[index].value;
        }
        index += 1;
    }
    assert!(
        tail_high >= APPEND_ONLY_BASE,
        "append-only method tail must exist"
    );
    let mut value = APPEND_ONLY_BASE;
    while value <= tail_high {
        let mut found = false;
        index = 0;
        while index < ids.len() {
            if ids[index].value == value {
                found = true;
                break;
            }
            index += 1;
        }
        assert!(found, "append-only method IDs must have no gaps");
        value += 1;
    }
}

method_ids! {
    (KILN_C99_METHOD_ARRAY_AT, 200, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_FIND, 201, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_FIND_INDEX, 202, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_SOME, 203, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_EVERY, 204, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_SORT, 205, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_FLAT, 206, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_ENTRIES, 207, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_KEYS, 208, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_REVERSE, 209, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_SHIFT, 210, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_UNSHIFT, 211, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_FILL, 212, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_LAST_INDEX_OF, 213, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_SPLICE, 214, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_COPY_WITHIN, 215, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_TO_STRING, 216, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_FLAT_MAP, 217, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_FIND_LAST, 218, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX, 219, ArrayLookup),
    (KILN_C99_METHOD_ARRAY_REDUCE_RIGHT, 220, ArrayLookup),
    (KILN_C99_METHOD_STRING_CODE_POINT_AT, 240, StringDispatch),
    (KILN_C99_METHOD_STRING_SUBSTR, 241, StringDispatch),
    (KILN_C99_METHOD_STRING_LOCALE_COMPARE, 242, StringDispatch),
    (KILN_C99_METHOD_STRING_NORMALIZE, 243, StringDispatch),
    (KILN_C99_METHOD_STRING_TO_STRING, 244, StringDispatch),
    (KILN_C99_METHOD_STRING_VALUE_OF, 245, StringDispatch),
    (KILN_C99_METHOD_STRING_MATCH, 246, StringDispatch),
    (KILN_C99_METHOD_STRING_MATCH_ALL, 247, StringDispatch),
    (KILN_C99_METHOD_STRING_SEARCH, 248, StringDispatch),
    (KILN_C99_METHOD_ERROR_TO_STRING, 249, StringDispatch),
    (KILN_C99_METHOD_PUSH, 1, ArrayLookup),
    (KILN_C99_METHOD_POP, 2, ArrayLookup),
    (KILN_C99_METHOD_JOIN, 3, ArrayLookup),
    (KILN_C99_METHOD_INDEX_OF, 4, ArrayLookup),
    (KILN_C99_METHOD_INCLUDES, 5, ArrayLookup),
    (KILN_C99_METHOD_SLICE, 6, ArrayLookup),
    (KILN_C99_METHOD_CONCAT, 7, ArrayLookup),
    (KILN_C99_METHOD_MAP, 8, ArrayLookup),
    (KILN_C99_METHOD_FILTER, 9, ArrayLookup),
    (KILN_C99_METHOD_FOR_EACH, 10, ArrayLookup),
    (KILN_C99_METHOD_REDUCE, 11, ArrayLookup),
    (KILN_C99_METHOD_STRING_SLICE, 12, StringDispatch),
    (KILN_C99_METHOD_SUBSTRING, 13, StringDispatch),
    (KILN_C99_METHOD_STRING_INDEX_OF, 14, StringDispatch),
    (KILN_C99_METHOD_LAST_INDEX_OF, 15, StringDispatch),
    (KILN_C99_METHOD_STRING_INCLUDES, 16, StringDispatch),
    (KILN_C99_METHOD_STARTS_WITH, 17, StringDispatch),
    (KILN_C99_METHOD_ENDS_WITH, 18, StringDispatch),
    (KILN_C99_METHOD_CHAR_AT, 19, StringDispatch),
    (KILN_C99_METHOD_CHAR_CODE_AT, 20, StringDispatch),
    (KILN_C99_METHOD_STRING_AT, 21, StringDispatch),
    (KILN_C99_METHOD_TRIM, 22, StringDispatch),
    (KILN_C99_METHOD_TRIM_START, 23, StringDispatch),
    (KILN_C99_METHOD_TRIM_END, 24, StringDispatch),
    (KILN_C99_METHOD_SPLIT, 25, StringDispatch),
    (KILN_C99_METHOD_REPEAT, 26, StringDispatch),
    (KILN_C99_METHOD_PAD_START, 27, StringDispatch),
    (KILN_C99_METHOD_PAD_END, 28, StringDispatch),
    (KILN_C99_METHOD_TO_UPPER_CASE, 29, StringDispatch),
    (KILN_C99_METHOD_TO_LOWER_CASE, 30, StringDispatch),
    (KILN_C99_METHOD_REPLACE, 31, StringDispatch),
    (KILN_C99_METHOD_REPLACE_ALL, 32, StringDispatch),
    (KILN_C99_METHOD_STRING_CONCAT, 33, StringDispatch),
    (KILN_C99_METHOD_MATH_FLOOR, 34, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_CEIL, 35, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_ROUND, 36, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_TRUNC, 37, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_ABS, 38, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_SIGN, 39, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_SQRT, 40, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_CBRT, 41, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_POW, 42, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_MIN, 43, Generic),
    (KILN_C99_METHOD_MATH_MAX, 44, Generic),
    (KILN_C99_METHOD_MATH_LOG, 45, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_LOG2, 46, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_LOG10, 47, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_EXP, 48, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_HYPOT, 49, Generic),
    (KILN_C99_METHOD_MATH_ATAN2, 50, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_SIN, 51, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_COS, 52, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_TAN, 53, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_ASIN, 54, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_ACOS, 55, Generic, MathNumber),
    (KILN_C99_METHOD_MATH_ATAN, 56, Generic, MathNumber),
    (KILN_C99_METHOD_IS_INTEGER, 57, Generic),
    (KILN_C99_METHOD_IS_SAFE_INTEGER, 58, Generic),
    (KILN_C99_METHOD_IS_NAN, 59, Generic),
    (KILN_C99_METHOD_IS_FINITE, 60, Generic),
    (KILN_C99_METHOD_OBJECT_KEYS, 61, Generic),
    (KILN_C99_METHOD_OBJECT_VALUES, 62, Generic),
    (KILN_C99_METHOD_OBJECT_ENTRIES, 63, Generic),
    (KILN_C99_METHOD_OBJECT_ASSIGN, 64, Generic),
    (KILN_C99_METHOD_HAS_OWN, 65, Generic),
    (KILN_C99_METHOD_IS_ARRAY, 66, Generic),
    (KILN_C99_METHOD_ARRAY_OF, 67, Generic),
    (KILN_C99_METHOD_OBJECT_CREATE, 68, Generic),
    (KILN_C99_METHOD_JSON_STRINGIFY, 69, Generic),
    (KILN_C99_METHOD_JSON_PARSE, 70, Generic),
    (KILN_C99_METHOD_GENERATOR_NEXT, 71, Generic),
    (KILN_C99_METHOD_GENERATOR_RETURN, 72, Generic),
    (KILN_C99_METHOD_GENERATOR_SELF, 73, Generic),
    (KILN_C99_METHOD_GET_TIME, 74, Generic),
    (KILN_C99_METHOD_GET_FULL_YEAR, 75, Generic),
    (KILN_C99_METHOD_GET_MONTH, 76, Generic),
    (KILN_C99_METHOD_GET_DATE, 77, Generic),
    (KILN_C99_METHOD_GET_DAY, 78, Generic),
    (KILN_C99_METHOD_GET_HOURS, 79, Generic),
    (KILN_C99_METHOD_GET_MINUTES, 80, Generic),
    (KILN_C99_METHOD_GET_SECONDS, 81, Generic),
    (KILN_C99_METHOD_GET_MILLISECONDS, 82, Generic),
    (KILN_C99_METHOD_GET_TIMEZONE_OFFSET, 83, Generic),
    (KILN_C99_METHOD_TO_ISO_STRING, 84, Generic),
    (KILN_C99_METHOD_TO_JSON, 85, Generic),
    (KILN_C99_METHOD_DATE_TO_STRING, 86, Generic),
    (KILN_C99_METHOD_DATE_NOW, 87, Generic),
    (KILN_C99_METHOD_DATE_UTC, 88, Generic),
    (KILN_C99_METHOD_DATE_PARSE, 89, Generic),
    (KILN_C99_METHOD_HAS_OWN_PROPERTY, 90, Generic),
    (KILN_C99_METHOD_OBJECT_TO_STRING, 91, Generic),
    (KILN_C99_METHOD_OBJECT_VALUE_OF, 92, Generic),
    (KILN_C99_METHOD_COLLECTION_GET, 93, Generic),
    (KILN_C99_METHOD_COLLECTION_SET, 94, Generic),
    (KILN_C99_METHOD_COLLECTION_ADD, 95, Generic),
    (KILN_C99_METHOD_COLLECTION_HAS, 96, Generic),
    (KILN_C99_METHOD_COLLECTION_DELETE, 97, Generic),
    (KILN_C99_METHOD_COLLECTION_CLEAR, 98, Generic),
    (KILN_C99_METHOD_COLLECTION_FOR_EACH, 99, Generic),
    (KILN_C99_METHOD_COLLECTION_KEYS, 100, Generic),
    (KILN_C99_METHOD_COLLECTION_VALUES, 101, Generic),
    (KILN_C99_METHOD_COLLECTION_ENTRIES, 102, Generic),
    (KILN_C99_METHOD_COLLECTION_SELF, 103, Generic),
    (KILN_C99_METHOD_ITERATOR_NEXT, 104, Generic),
    (KILN_C99_METHOD_ITERATOR_SELF, 105, Generic),
    (KILN_C99_METHOD_ARRAY_VALUES, 106, Generic),
    (KILN_C99_METHOD_THEN, 107, Generic),
    (KILN_C99_METHOD_CATCH, 108, Generic),
    (KILN_C99_METHOD_FINALLY, 109, Generic),
    (KILN_C99_METHOD_PROMISE_RESOLVE, 110, Generic),
    (KILN_C99_METHOD_PROMISE_REJECT, 111, Generic),
    (KILN_C99_METHOD_PROMISE_ALL, 112, Generic),
    (KILN_C99_METHOD_PROMISE_RACE, 113, Generic),
    (KILN_C99_METHOD_ASYNC_RESUME, 114, Generic),
    (KILN_C99_METHOD_ASYNC_THROW, 115, Generic),
    (KILN_C99_METHOD_ALL_STEP, 116, Generic),
    (KILN_C99_METHOD_PARSE_INT, 117, Generic),
    (KILN_C99_METHOD_PARSE_FLOAT, 118, Generic),
    (KILN_C99_METHOD_GLOBAL_IS_NAN, 119, Generic),
    (KILN_C99_METHOD_GLOBAL_IS_FINITE, 120, Generic),
    (KILN_C99_METHOD_NUMBER_TO_STRING, 121, Generic),
    (KILN_C99_METHOD_NUMBER_TO_FIXED, 122, Generic),
    (KILN_C99_METHOD_NUMBER_TO_EXPONENTIAL, 197, Generic),
    (KILN_C99_METHOD_NUMBER_TO_PRECISION, 198, Generic),
    (KILN_C99_METHOD_NUMBER_VALUE_OF, 123, Generic),
    (KILN_C99_METHOD_ESCAPE, 124, Generic),
    (KILN_C99_METHOD_UNESCAPE, 125, Generic),
    (KILN_C99_METHOD_ENCODE_URI, 126, Generic),
    (KILN_C99_METHOD_ENCODE_URI_COMPONENT, 127, Generic),
    (KILN_C99_METHOD_DECODE_URI, 128, Generic),
    (KILN_C99_METHOD_DECODE_URI_COMPONENT, 129, Generic),
    (KILN_C99_METHOD_QUEUE_MICROTASK, 130, Generic),
    (KILN_C99_METHOD_SET_TIMEOUT, 131, Generic),
    (KILN_C99_METHOD_SET_INTERVAL, 132, Generic),
    (KILN_C99_METHOD_CLEAR_TIMEOUT, 133, Generic),
    (KILN_C99_METHOD_CLEAR_INTERVAL, 134, Generic),
    (KILN_C99_METHOD_PROMISE_ALL_SETTLED, 135, Generic),
    (KILN_C99_METHOD_PROMISE_ANY, 136, Generic),
    (KILN_C99_METHOD_PROMISE_WITH_RESOLVERS, 411, Generic),
    (KILN_C99_METHOD_NODE_TTY_ISATTY, 412, Direct),
    (KILN_C99_METHOD_TYPED_TO_STRING_TAG, 413, Direct),
    (KILN_C99_METHOD_REJECT_STEP, 137, Generic),
    (KILN_C99_METHOD_FINALLY_PASS, 138, Generic),
    (KILN_C99_METHOD_FINALLY_RAISE, 139, Generic),
    (KILN_C99_METHOD_FINALLY_RESTORE, 140, Generic),
    (KILN_C99_METHOD_FINALLY_RERAISE, 141, Generic),
    (KILN_C99_METHOD_ADOPT_STEP, 142, Generic),
    (KILN_C99_METHOD_OBJECT_FREEZE, 143, Generic),
    (KILN_C99_METHOD_OBJECT_IS_FROZEN, 144, Generic),
    (KILN_C99_METHOD_OBJECT_PREVENT_EXTENSIONS, 436, Generic),
    (KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE, 437, Generic),
    (KILN_C99_METHOD_FUNCTION_CALL, 145, Generic),
    (KILN_C99_METHOD_FUNCTION_APPLY, 146, Generic),
    (KILN_C99_METHOD_FUNCTION_BIND, 147, Generic),
    (KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF, 148, Generic),
    (KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF, 149, Generic),
    (KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_NAMES, 150, Generic),
    (KILN_C99_METHOD_OBJECT_FROM_ENTRIES, 151, Generic),
    (KILN_C99_METHOD_ARRAY_FROM, 152, Generic),
    (KILN_C99_METHOD_IS_PROTOTYPE_OF, 153, Generic),
    (KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE, 154, Generic),
    (KILN_C99_METHOD_STRING_RAW, 155, Generic),
    (KILN_C99_METHOD_STRING_FROM_CHAR_CODE, 442, Generic),
    (KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY, 156, Generic),
    (KILN_C99_METHOD_OBJECT_DEFINE_PROPERTIES, 157, Generic),
    (KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTOR, 158, Generic),
    (KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTORS, 159, Generic),
    (KILN_C99_METHOD_OBJECT_IS, 185, Generic),
    (KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING, 196, Generic),
    (KILN_C99_METHOD_ERROR_CAPTURE_STACK_TRACE, 186, Generic),
    (KILN_C99_METHOD_REFLECT_OWN_KEYS, 187, Generic),
    (KILN_C99_METHOD_REFLECT_GET, 188, Generic),
    (KILN_C99_METHOD_REFLECT_SET, 189, Generic),
    (KILN_C99_METHOD_REFLECT_HAS, 190, Generic),
    (KILN_C99_METHOD_REFLECT_DELETE_PROPERTY, 191, Generic),
    (KILN_C99_METHOD_REFLECT_APPLY, 192, Generic),
    (KILN_C99_METHOD_REFLECT_CONSTRUCT, 193, Generic),
    (KILN_C99_METHOD_PROXY_REVOCABLE, 194, Generic),
    (KILN_C99_METHOD_PROXY_REVOKE, 195, Generic),
    (KILN_C99_METHOD_MATH_RANDOM, 160, Generic),
    (KILN_C99_METHOD_MATH_SINH, 161, Generic),
    (KILN_C99_METHOD_MATH_COSH, 162, Generic),
    (KILN_C99_METHOD_MATH_TANH, 163, Generic),
    (KILN_C99_METHOD_MATH_ASINH, 164, Generic),
    (KILN_C99_METHOD_MATH_ACOSH, 165, Generic),
    (KILN_C99_METHOD_MATH_ATANH, 166, Generic),
    (KILN_C99_METHOD_MATH_LOG1P, 167, Generic),
    (KILN_C99_METHOD_MATH_EXPM1, 168, Generic),
    (KILN_C99_METHOD_MATH_FROUND, 169, Generic),
    (KILN_C99_METHOD_MATH_CLZ32, 170, Generic),
    (KILN_C99_METHOD_MATH_IMUL, 171, Generic),
    (KILN_C99_METHOD_REGEXP_TEST, 172, Generic),
    (KILN_C99_METHOD_REGEXP_EXEC, 173, Generic),
    (KILN_C99_METHOD_REGEXP_TO_STRING, 174, Generic),
    (KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT, 175, Generic),
    (KILN_C99_METHOD_PRIMITIVE_TO_STRING, 176, Generic),
    (KILN_C99_METHOD_TYPED_SUBARRAY, 230, Direct),
    (KILN_C99_METHOD_TYPED_SET, 231, Direct),
    (KILN_C99_METHOD_TYPED_FILL, 232, Direct),
    (KILN_C99_METHOD_TYPED_OF, 233, Direct),
    (KILN_C99_METHOD_TYPED_FROM, 234, Direct),
    (KILN_C99_METHOD_ARRAY_GENERIC_MAP, 235, Direct),
    (KILN_C99_METHOD_ARRAY_GENERIC_FILTER, 236, Direct),
    (KILN_C99_METHOD_NODE_EVENT_ON, 260, NodeEvent),
    (KILN_C99_METHOD_NODE_EVENT_ONCE, 261, NodeEvent),
    (KILN_C99_METHOD_NODE_EVENT_EMIT, 262, NodeEvent),
    (KILN_C99_METHOD_NODE_EVENT_LISTENERS, 263, NodeEvent),
    (KILN_C99_METHOD_NODE_EVENT_REMOVE_LISTENER, 264, NodeEvent),
    (KILN_C99_METHOD_NODE_EVENT_REMOVE_ALL_LISTENERS, 265, NodeEvent),
    (KILN_C99_METHOD_NODE_EVENT_SET_MAX_LISTENERS, 266, NodeEvent),
    (KILN_C99_METHOD_NODE_EVENT_EMITTER, 267, NodeEvent),
    (KILN_C99_METHOD_NODE_NET_CREATE_SERVER, 268, NodeNet),
    (KILN_C99_METHOD_NODE_SERVER_LISTEN, 269, NodeNet, NodeServer),
    (KILN_C99_METHOD_NODE_SERVER_CLOSE, 270, NodeNet, NodeServer),
    (KILN_C99_METHOD_NODE_SERVER_ADDRESS, 271, NodeNet, NodeServer),
    (KILN_C99_METHOD_NODE_SOCKET_WRITE, 272, NodeNet),
    (KILN_C99_METHOD_NODE_SOCKET_END, 273, NodeNet),
    (KILN_C99_METHOD_NODE_SOCKET_DESTROY, 274, NodeNet),
    (KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER, 275, NodeHttp),
    (KILN_C99_METHOD_NODE_RESPONSE_SET_HEADER, 276, NodeHttp),
    (KILN_C99_METHOD_NODE_RESPONSE_WRITE_HEAD, 277, NodeHttp),
    (KILN_C99_METHOD_NODE_RESPONSE_WRITE, 278, NodeHttp),
    (KILN_C99_METHOD_NODE_RESPONSE_END, 279, NodeHttp),
    (KILN_C99_METHOD_NODE_HTTP_GET, 280, NodeHttp),
    (KILN_C99_METHOD_NODE_HTTP_REQUEST, 281, NodeHttp),
    (KILN_C99_METHOD_NODE_CLIENT_REQUEST_WRITE, 282, NodeHttp),
    (KILN_C99_METHOD_NODE_CLIENT_REQUEST_END, 283, NodeHttp),
    /* Three groups, and no precedence between them: `kiln_c99_invoke_method` routes this
     * code by receiver before it consults any group. See the comment there. */
    (KILN_C99_METHOD_NODE_READABLE_SET_ENCODING, 284, NodeHttp, NodeNet, NodeStream),
    (KILN_C99_METHOD_NODE_NET_CONNECT, 338, Direct, NodeNet),
    (KILN_C99_METHOD_URL_SEARCH_PARAMS_GET, 339, Direct),
    (KILN_C99_METHOD_BUFFER_FROM, 340, Buffer),
    (KILN_C99_METHOD_BUFFER_ALLOC, 341, Buffer),
    (KILN_C99_METHOD_BUFFER_IS_BUFFER, 342, Buffer),
    (KILN_C99_METHOD_BUFFER_BYTE_LENGTH, 343, Buffer),
    (KILN_C99_METHOD_BUFFER_CONCAT, 344, Buffer),
    (KILN_C99_METHOD_BUFFER_TO_STRING, 345, Buffer),
    (KILN_C99_METHOD_NODE_UTIL_DEBUGLOG, 346, NodeUtil),
    (KILN_C99_METHOD_NODE_UTIL_NOOP, 347, NodeUtil),
    (KILN_C99_METHOD_NODE_UTIL_DEPRECATE, 348, NodeUtil),
    (KILN_C99_METHOD_NODE_UTIL_INSPECT, 349, NodeUtil),
    (KILN_C99_METHOD_NODE_UTIL_INHERITS, 350, NodeUtil),
    (KILN_C99_METHOD_NODE_UTIL_FORMAT, 351, NodeUtil),
    (KILN_C99_METHOD_NODE_DIAGNOSTICS_CHANNEL, 352, NodeDiagnostics),
    (KILN_C99_METHOD_NODE_DIAGNOSTICS_TRACING_CHANNEL, 353, NodeDiagnostics),
    (KILN_C99_METHOD_NODE_DIAGNOSTICS_PUBLISH, 354, NodeDiagnostics),
    (KILN_C99_METHOD_NODE_DIAGNOSTICS_RUN_STORES, 355, NodeDiagnostics),
    (KILN_C99_METHOD_NODE_HTTPS_CREATE_SERVER, 356, Direct),
    (KILN_C99_METHOD_NODE_HTTP2_CREATE_SERVER, 357, Direct),
    (KILN_C99_METHOD_NODE_HTTP2_CREATE_SECURE_SERVER, 358, Direct),
    (KILN_C99_METHOD_NODE_DNS_LOOKUP, 359, Direct),
    (KILN_C99_METHOD_NODE_OS_HOSTNAME, 360, Direct),
    (KILN_C99_METHOD_NODE_OS_NETWORK_INTERFACES, 361, Direct),
    (KILN_C99_METHOD_NODE_STREAM, 362, NodeStream),
    (KILN_C99_METHOD_NODE_READABLE, 363, NodeStream),
    (KILN_C99_METHOD_NODE_WRITABLE, 364, NodeStream),
    (KILN_C99_METHOD_NODE_STREAM_ADD_ABORT_SIGNAL, 365, NodeStream),
    (KILN_C99_METHOD_NODE_STREAM_FINISHED, 366, NodeStream),
    (KILN_C99_METHOD_NODE_READABLE_PUSH, 367, NodeStream),
    (KILN_C99_METHOD_NODE_READABLE_READ, 368, NodeStream),
    (KILN_C99_METHOD_NODE_READABLE_DESTROY, 369, NodeStream),
    (KILN_C99_METHOD_NODE_READABLE_PAUSE, 370, NodeStream),
    (KILN_C99_METHOD_NODE_READABLE_RESUME, 371, NodeStream),
    (KILN_C99_METHOD_NODE_READABLE_PIPE, 372, NodeStream),
    (KILN_C99_METHOD_NODE_READABLE_UNPIPE, 373, NodeStream),
    (KILN_C99_METHOD_NODE_WRITABLE_WRITE, 374, NodeStream),
    (KILN_C99_METHOD_NODE_WRITABLE_END, 375, NodeStream),
    (KILN_C99_METHOD_NODE_WRITABLE_DESTROY, 376, NodeStream),
    (KILN_C99_METHOD_NODE_PERFORMANCE_NOW, 377, Direct),
    (KILN_C99_METHOD_NODE_FS_EXISTS_SYNC, 378, Direct),
    (KILN_C99_METHOD_NODE_FS_READ_FILE, 379, Direct),
    (KILN_C99_METHOD_NODE_FS_STAT, 380, Direct),
    (KILN_C99_METHOD_NODE_FS_WRITE_FILE, 381, Direct),
    (KILN_C99_METHOD_NODE_PATH_IS_ABSOLUTE, 382, NodePath),
    (KILN_C99_METHOD_NODE_PATH_JOIN, 383, NodePath),
    (KILN_C99_METHOD_NODE_PATH_RESOLVE, 384, NodePath),
    (KILN_C99_METHOD_NODE_PATH_DIRNAME, 438, NodePath),
    (KILN_C99_METHOD_NODE_PATH_BASENAME, 439, NodePath),
    (KILN_C99_METHOD_NODE_PATH_EXTNAME, 440, NodePath),
    (KILN_C99_METHOD_NODE_PATH_RELATIVE, 441, NodePath),
    (KILN_C99_METHOD_NODE_ASSERT_OK, 385, Direct),
    (KILN_C99_METHOD_NODE_WORKER_CONSTRUCTOR, 386, Direct),
    (KILN_C99_METHOD_NODE_MODULE_CREATE_REQUIRE, 387, Direct),
    (KILN_C99_METHOD_NODE_URL_FILE_URL_TO_PATH, 388, Direct),
    (KILN_C99_METHOD_NODE_URL_PARSE, 389, Direct),
    (KILN_C99_METHOD_NODE_URL_PATH_TO_FILE_URL, 390, Direct),
    (KILN_C99_METHOD_NODE_ASYNC_RESOURCE, 391, Direct),
    (KILN_C99_METHOD_NODE_ASYNC_RESOURCE_BIND, 392, Direct),
    (KILN_C99_METHOD_NODE_ASYNC_RESOURCE_DESTROY, 393, Direct),
    (KILN_C99_METHOD_NODE_CRYPTO_RANDOM_UUID, 394, Direct),
    (KILN_C99_METHOD_NODE_CRYPTO_CREATE_HASH, 395, Direct),
    (KILN_C99_METHOD_PROCESS_NEXT_TICK, 396, Direct),
    (KILN_C99_METHOD_PROCESS_HRTIME, 397, Direct),
    (KILN_C99_METHOD_PROCESS_HRTIME_BIGINT, 398, Direct),
    (KILN_C99_METHOD_PROCESS_CWD, 399, Direct),
    (KILN_C99_METHOD_ATOMICS_UNAVAILABLE, 400, Direct),
    (KILN_C99_METHOD_SET_IMMEDIATE, 401, Direct),
    (KILN_C99_METHOD_CLEAR_IMMEDIATE, 402, Direct),
    (KILN_C99_METHOD_NODE_HTTP_SERVER, 403, Direct),
    (KILN_C99_METHOD_NODE_SERVER_SET_TIMEOUT, 404, Direct, NodeServer, NodeNet),
    (KILN_C99_METHOD_NODE_RESPONSE_GET_HEADER, 405, Direct, NodeResponse),
    (KILN_C99_METHOD_NODE_RESPONSE_GET_HEADERS, 406, Direct, NodeResponse),
    (KILN_C99_METHOD_NODE_RESPONSE_HAS_HEADER, 407, Direct, NodeResponse),
    (KILN_C99_METHOD_NODE_RESPONSE_REMOVE_HEADER, 408, Direct, NodeResponse),
    (KILN_C99_METHOD_SYMBOL_FOR, 409, Direct),
    (KILN_C99_METHOD_SYMBOL_KEY_FOR, 410, Direct),
    (KILN_C99_METHOD_NODE_STAT_IS_FILE, 414, Direct),
    (KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY, 415, Direct),
    (KILN_C99_METHOD_ABORT_CONTROLLER_ABORT, 416, Generic),
    (KILN_C99_METHOD_ABORT_SIGNAL_ABORT, 417, Generic),
    (KILN_C99_METHOD_ABORT_SIGNAL_LISTEN, 418, Generic),
    (KILN_C99_METHOD_ABORT_SIGNAL_UNLISTEN, 419, Generic),
    (KILN_C99_METHOD_ABORT_SIGNAL_THROW, 420, Generic),
    (KILN_C99_METHOD_ABORT_SIGNAL_TIMEOUT, 421, Generic),
    (KILN_C99_METHOD_ABORT_SIGNAL_ANY, 422, Generic),
    (KILN_C99_METHOD_ABORT_SIGNAL_SETTLE, 423, Generic),
    (KILN_C99_METHOD_ABORT_SIGNAL_FOLLOW, 424, Generic),
    (KILN_C99_METHOD_TEXT_ENCODER_ENCODE, 425, Generic),
    (KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE, 426, Direct, NodeHttp),
    (KILN_C99_METHOD_NODE_QUERYSTRING_PARSE, 427, Direct),
    (KILN_C99_METHOD_FUNCTION_TO_STRING, 428, Generic),
    (KILN_C99_METHOD_ARRAY_BUFFER_IS_VIEW, 429, Generic),
    (KILN_C99_METHOD_THROW_TYPE_ERROR, 430, Generic),
    /* `console.log` reached as a *value* rather than through the `console.log(x)` shape the
     * compiler recognises. `Direct` and attached to the console object by name, exactly as the
     * `Atomics` methods are: nothing looks these up by name through a group range, so a `log`
     * on some other receiver cannot find them. */
    (KILN_C99_METHOD_CONSOLE_LOG, 431, Direct),
    (KILN_C99_METHOD_CONSOLE_UNAVAILABLE, 432, Direct),
    (KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_SYMBOLS, 433, Generic),
    /* The two halves of `req.headers`. They are a method pair rather than a stored object so
     * that the header table is built only if a program reads it -- see the accessor installed in
     * `kiln_c99_node_http_event`. */
    (KILN_C99_METHOD_NODE_REQUEST_HEADERS_GET, 434, Direct, NodeHttp),
    (KILN_C99_METHOD_NODE_REQUEST_HEADERS_SET, 435, Direct, NodeHttp),
    (KILN_C99_METHOD_TEMPORAL_FROM, 285, Temporal, TemporalFrom),
    (KILN_C99_METHOD_TEMPORAL_FROM_LAST, 292, Temporal, TemporalFrom),
    (KILN_C99_METHOD_TEMPORAL_COMPARE, 293, Temporal, TemporalCompare),
    (KILN_C99_METHOD_TEMPORAL_COMPARE_LAST, 300, Temporal, TemporalCompare),
    (KILN_C99_METHOD_TEMPORAL_TO_STRING, 301, Temporal),
    (KILN_C99_METHOD_TEMPORAL_TO_JSON, 302, Temporal),
    (KILN_C99_METHOD_TEMPORAL_VALUE_OF, 303, Temporal),
    (KILN_C99_METHOD_TEMPORAL_NOW_TIME_ZONE_ID, 304, Temporal),
    (KILN_C99_METHOD_TEMPORAL_NOW_READ, 305, Temporal, TemporalNowRead),
    (KILN_C99_METHOD_TEMPORAL_NOW_READ_LAST, 312, Temporal, TemporalNowRead),
    (KILN_C99_METHOD_TEMPORAL_OP, 313, Temporal, TemporalOp),
    (KILN_C99_METHOD_TEMPORAL_OP_LAST, 335, Temporal, TemporalOp),
    (KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_MILLISECONDS, 336, Temporal),
    (KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS, 337, Temporal),
    (KILN_C99_METHOD_TEMPORAL_LAST, 337, Temporal),
    (KILN_C99_METHOD_SET_TIME, 177, Generic, DateSetter),
    (KILN_C99_METHOD_SET_FULL_YEAR, 178, Generic, DateSetter),
    (KILN_C99_METHOD_SET_MONTH, 179, Generic, DateSetter),
    (KILN_C99_METHOD_SET_DATE, 180, Generic, DateSetter),
    (KILN_C99_METHOD_SET_HOURS, 181, Generic, DateSetter),
    (KILN_C99_METHOD_SET_MINUTES, 182, Generic, DateSetter),
    (KILN_C99_METHOD_SET_SECONDS, 183, Generic, DateSetter),
    (KILN_C99_METHOD_SET_MILLISECONDS, 184, Generic, DateSetter),
}

/// The hook a host installs when it lets a program make a second realm.
///
/// Spelled here rather than shared with `kiln-interpreter` because this backend deliberately
/// takes no dependency on that crate: what it needs is the *name* a program would reach for, and
/// a name is a fact about the program, not about the interpreter's tables.
const HOST_CREATE_REALM: &str = "$262CreateRealm";

/// The hook a host installs when it lets a program evaluate Script source at run time.
///
/// Refused for the same reason `DirectEval` is: this backend has no compiler to reach at run
/// time. Named separately from `eval` because it is a different name in the program.
const HOST_EVAL_SCRIPT: &str = "$262EvalScript";

/// The hooks a host installs when it lets a program start a second agent.
///
/// Refused for a sharper reason than the realm hook is. A second agent is a second thread with a
/// realm of its own, blocking `Atomics.wait`, and a scheduler that decides the interleaving; this
/// backend emits one C translation unit with one global object and no scheduler at all. A
/// compiled program that reached these would start nothing, wait on nobody, and answer every
/// `Atomics.wait` "timed-out" while the interpreter answered "ok" -- a silent divergence between
/// the two engines, which is the one failure `scripts/native.sh` exists to prevent. Refusing by
/// name says which capability is missing, and the differential harness counts it as refused
/// rather than as diverged.
const HOST_AGENT_HOOKS: [&str; 8] = [
    "$262AgentStart",
    "$262AgentBroadcast",
    "$262AgentGetReport",
    "$262AgentSleep",
    "$262AgentMonotonicNow",
    "$262AgentReceiveBroadcast",
    "$262AgentReport",
    "$262AgentLeaving",
];

/// Whether this name is a host hook this backend has no way to honour.
fn refused_host_hook(name: &str) -> bool {
    name == HOST_CREATE_REALM || name == HOST_EVAL_SCRIPT || HOST_AGENT_HOOKS.contains(&name)
}
/// The constructor a program reaches for when it wants a realm it can evaluate in.
///
/// Refused for the same reason the two above are, and it matters more here because
/// `ShadowRealm` is a *language* built-in rather than a host hook: a program can name it without
/// asking anyone's permission. This backend builds one global object, once, in C. A compiled
/// `new ShadowRealm()` would therefore hand back a realm whose intrinsics were secretly this
/// one's -- so `r.evaluate("Array") === Array` would be true, every wrapped function would be
/// the function it wrapped, and the boundary the whole feature exists to enforce would not
/// exist. Refusing by name says which construct is missing; answering would be a confident
/// wrong answer about object identity.
const SHADOW_REALM: &str = "ShadowRealm";

const NATIVE_CONSTRUCTORS: &[NativeConstructor] = &[
    native_constructor!(KILN_C99_METHOD_NODE_EVENT_EMITTER, None, EventEmitter),
    native_constructor!(
        KILN_C99_METHOD_NODE_READABLE,
        Some(method_value!(KILN_C99_METHOD_NODE_EVENT_EMITTER)),
        Readable
    ),
    native_constructor!(
        KILN_C99_METHOD_NODE_WRITABLE,
        Some(method_value!(KILN_C99_METHOD_NODE_EVENT_EMITTER)),
        Writable
    ),
    native_constructor!(
        KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE,
        Some(method_value!(KILN_C99_METHOD_NODE_WRITABLE)),
        ServerResponse
    ),
];

const _: () = validate_method_ids(METHOD_IDS);
const _: () = validate_native_constructors(NATIVE_CONSTRUCTORS, METHOD_IDS);

/// One member of one built-in's *static* surface: what `Number.MAX_VALUE` and `Object.keys` name.
///
/// ONE TABLE, TWO EMITTERS (ADR 0043). Before [`BUILTIN_SURFACE`] the surface was written down
/// twice -- once in the interpreter's Rust, once as hand-written `kiln_c99_same_name` chains in
/// the emitted C -- and nothing compared the two. `Number` had fifteen statics on one side and
/// five on the other, and `Number.MAX_VALUE` is the first property a bundled `zod` reads, so its
/// absence refused the whole module. Those were not four bugs; they were one shape of bug with an
/// unbounded supply.
///
/// The rows below are now the only place the native surface is written down. The read path, the
/// call path and `Object.getOwnPropertyNames` are all generated from them, which is what makes a
/// member that is *implemented but unreachable* inexpressible: reachability is the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinMember {
    /// The global that owns it, spelled as a program spells it: `"Number"`, `"Math"`.
    pub owner: &'static str,
    /// The member's own name. A `@@` prefix means symbol-keyed, the same spelling the emitted C
    /// uses for a reserved store key.
    pub name: &'static str,
}

macro_rules! builtin_member_row {
    ($builtin:ident, $name:literal, code($code:ident)) => {
        concat!(
            "    {",
            stringify!($builtin),
            ", KILN_C99_MEMBER_METHOD, ",
            stringify!($code),
            ", 0.0, \"",
            $name,
            "\", NULL},\n"
        )
    };
    ($builtin:ident, $name:literal, num($value:literal)) => {
        concat!(
            "    {",
            stringify!($builtin),
            ", KILN_C99_MEMBER_NUMBER, 0, ",
            $value,
            ", \"",
            $name,
            "\", NULL},\n"
        )
    };
    ($builtin:ident, $name:literal, text($value:literal)) => {
        concat!(
            "    {",
            stringify!($builtin),
            ", KILN_C99_MEMBER_TEXT, 0, 0.0, \"",
            $name,
            "\", \"",
            $value,
            "\"},\n"
        )
    };
    ($builtin:ident, $name:literal, sym($key:literal)) => {
        concat!(
            "    {",
            stringify!($builtin),
            ", KILN_C99_MEMBER_SYMBOL, 0, 0.0, \"",
            $name,
            "\", \"",
            $key,
            "\"},\n"
        )
    };
    ($builtin:ident, $name:literal, ctor($value:ident)) => {
        concat!(
            "    {",
            stringify!($builtin),
            ", KILN_C99_MEMBER_BUILTIN, ",
            stringify!($value),
            ", 0.0, \"",
            $name,
            "\", NULL},\n"
        )
    };
}

/// The single declaration of every table-driven built-in's static surface.
///
/// A row is `(C builtin constant, the global that names it, the member's name, what it answers)`.
/// `code(...)` is a method the program calls, `num(...)` and `text(...)` a value it reads,
/// `sym(...)` a well-known symbol, and `ctor(...)` a built-in handed back by name --
/// `Array[Symbol.species]` is `Array`.
///
/// Rows are declared in the interpreter's own order, because that order is what
/// `Object.getOwnPropertyNames` reports on both sides and agreeing costs nothing here.
macro_rules! builtin_surface {
    ($(($builtin:ident, $owner:literal, $name:literal, $kind:tt $args:tt)),* $(,)?) => {
        /// Every member the emitted C answers for, by owner. See [`BuiltinMember`].
        pub const BUILTIN_SURFACE: &[BuiltinMember] = &[
            $(BuiltinMember { owner: $owner, name: $name },)*
        ];

        macro_rules! emit_builtin_surface_rows {
            () => { concat!($(builtin_member_row!($builtin, $name, $kind $args)),*) };
        }
    };
}

builtin_surface! {
    (KILN_C99_BUILTIN_OBJECT, "Object", "keys", code(KILN_C99_METHOD_OBJECT_KEYS)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "getOwnPropertyNames",
     code(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_NAMES)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "assign", code(KILN_C99_METHOD_OBJECT_ASSIGN)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "preventExtensions",
     code(KILN_C99_METHOD_OBJECT_PREVENT_EXTENSIONS)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "isExtensible",
     code(KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "entries", code(KILN_C99_METHOD_OBJECT_ENTRIES)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "values", code(KILN_C99_METHOD_OBJECT_VALUES)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "freeze", code(KILN_C99_METHOD_OBJECT_FREEZE)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "isFrozen", code(KILN_C99_METHOD_OBJECT_IS_FROZEN)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "create", code(KILN_C99_METHOD_OBJECT_CREATE)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "defineProperty",
     code(KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "fromEntries", code(KILN_C99_METHOD_OBJECT_FROM_ENTRIES)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "hasOwn", code(KILN_C99_METHOD_HAS_OWN)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "is", code(KILN_C99_METHOD_OBJECT_IS)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "getOwnPropertySymbols",
     code(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_SYMBOLS)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "defineProperties",
     code(KILN_C99_METHOD_OBJECT_DEFINE_PROPERTIES)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "getOwnPropertyDescriptor",
     code(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTOR)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "getOwnPropertyDescriptors",
     code(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTORS)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "setPrototypeOf",
     code(KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "getPrototypeOf",
     code(KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF)),

    (KILN_C99_BUILTIN_ARRAY, "Array", "isArray", code(KILN_C99_METHOD_IS_ARRAY)),
    (KILN_C99_BUILTIN_ARRAY, "Array", "from", code(KILN_C99_METHOD_ARRAY_FROM)),
    (KILN_C99_BUILTIN_ARRAY, "Array", "of", code(KILN_C99_METHOD_ARRAY_OF)),
    // `Array[Symbol.species]` is `Array`. It read `undefined` natively and a function in the
    // interpreter, with no refusal in between -- the silent half of the drift, and the exact
    // shape `typeof X === "undefined"` feature-detection walks into.
    (KILN_C99_BUILTIN_ARRAY, "Array", "@@species", ctor(KILN_C99_BUILTIN_ARRAY)),

    (KILN_C99_BUILTIN_STRING, "String", "raw", code(KILN_C99_METHOD_STRING_RAW)),
    (KILN_C99_BUILTIN_STRING, "String", "fromCharCode", code(KILN_C99_METHOD_STRING_FROM_CHAR_CODE)),

    (KILN_C99_BUILTIN_NUMBER, "Number", "isInteger", code(KILN_C99_METHOD_IS_INTEGER)),
    (KILN_C99_BUILTIN_NUMBER, "Number", "isSafeInteger", code(KILN_C99_METHOD_IS_SAFE_INTEGER)),
    (KILN_C99_BUILTIN_NUMBER, "Number", "isNaN", code(KILN_C99_METHOD_IS_NAN)),
    (KILN_C99_BUILTIN_NUMBER, "Number", "isFinite", code(KILN_C99_METHOD_IS_FINITE)),
    // `Number.parseInt` and the global `parseInt` are the same function in the language, so they
    // are the same code here.
    (KILN_C99_BUILTIN_NUMBER, "Number", "parseInt", code(KILN_C99_METHOD_PARSE_INT)),
    (KILN_C99_BUILTIN_NUMBER, "Number", "parseFloat", code(KILN_C99_METHOD_PARSE_FLOAT)),
    (KILN_C99_BUILTIN_NUMBER, "Number", "MAX_SAFE_INTEGER", num("9007199254740991.0")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "MIN_SAFE_INTEGER", num("-9007199254740991.0")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "MAX_VALUE", num("1.7976931348623157e308")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "MIN_VALUE", num("5e-324")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "EPSILON", num("2.220446049250313e-16")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "POSITIVE_INFINITY", num("INFINITY")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "NEGATIVE_INFINITY", num("-INFINITY")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "NaN", num("NAN")),

    (KILN_C99_BUILTIN_DATE, "Date", "now", code(KILN_C99_METHOD_DATE_NOW)),
    (KILN_C99_BUILTIN_DATE, "Date", "UTC", code(KILN_C99_METHOD_DATE_UTC)),
    (KILN_C99_BUILTIN_DATE, "Date", "parse", code(KILN_C99_METHOD_DATE_PARSE)),

    (KILN_C99_BUILTIN_PROMISE, "Promise", "resolve", code(KILN_C99_METHOD_PROMISE_RESOLVE)),
    (KILN_C99_BUILTIN_PROMISE, "Promise", "reject", code(KILN_C99_METHOD_PROMISE_REJECT)),
    (KILN_C99_BUILTIN_PROMISE, "Promise", "all", code(KILN_C99_METHOD_PROMISE_ALL)),
    (KILN_C99_BUILTIN_PROMISE, "Promise", "allSettled", code(KILN_C99_METHOD_PROMISE_ALL_SETTLED)),
    (KILN_C99_BUILTIN_PROMISE, "Promise", "race", code(KILN_C99_METHOD_PROMISE_RACE)),
    (KILN_C99_BUILTIN_PROMISE, "Promise", "any", code(KILN_C99_METHOD_PROMISE_ANY)),
    (KILN_C99_BUILTIN_PROMISE, "Promise", "withResolvers",
     code(KILN_C99_METHOD_PROMISE_WITH_RESOLVERS)),

    (KILN_C99_BUILTIN_JSON, "JSON", "parse", code(KILN_C99_METHOD_JSON_PARSE)),
    (KILN_C99_BUILTIN_JSON, "JSON", "stringify", code(KILN_C99_METHOD_JSON_STRINGIFY)),
    (KILN_C99_BUILTIN_JSON, "JSON", "@@toStringTag", text("JSON")),

    (KILN_C99_BUILTIN_REFLECT, "Reflect", "deleteProperty",
     code(KILN_C99_METHOD_REFLECT_DELETE_PROPERTY)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "apply", code(KILN_C99_METHOD_REFLECT_APPLY)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "construct", code(KILN_C99_METHOD_REFLECT_CONSTRUCT)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "get", code(KILN_C99_METHOD_REFLECT_GET)),
    // Really the same operation `Object` carries, so it is the same code; the rest differ from
    // their `Object` namesakes in what they answer and are their own.
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "getOwnPropertyDescriptor",
     code(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTOR)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "has", code(KILN_C99_METHOD_REFLECT_HAS)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "ownKeys", code(KILN_C99_METHOD_REFLECT_OWN_KEYS)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "set", code(KILN_C99_METHOD_REFLECT_SET)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "@@toStringTag", text("Reflect")),

    (KILN_C99_BUILTIN_PROXY, "Proxy", "revocable", code(KILN_C99_METHOD_PROXY_REVOCABLE)),

    (KILN_C99_BUILTIN_ARRAY_BUFFER, "ArrayBuffer", "isView",
     code(KILN_C99_METHOD_ARRAY_BUFFER_IS_VIEW)),

    (KILN_C99_BUILTIN_PERFORMANCE, "performance", "now",
     code(KILN_C99_METHOD_NODE_PERFORMANCE_NOW)),

    // Every error constructor carries `captureStackTrace`, which `kiln_c99_namespace_method`
    // answers ahead of the table rather than repeating in a row per kind. The row is here so that
    // `Object.getOwnPropertyNames(Error)` and the drift test can both see it.
    (KILN_C99_BUILTIN_ERROR, "Error", "captureStackTrace",
     code(KILN_C99_METHOD_ERROR_CAPTURE_STACK_TRACE)),

    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "for", code(KILN_C99_METHOD_SYMBOL_FOR)),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "keyFor", code(KILN_C99_METHOD_SYMBOL_KEY_FOR)),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "asyncIterator", sym("@@asyncIterator")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "hasInstance", sym("@@hasInstance")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "isConcatSpreadable", sym("@@isConcatSpreadable")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "iterator", sym("@@iterator")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "match", sym("@@match")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "matchAll", sym("@@matchAll")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "replace", sym("@@replace")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "search", sym("@@search")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "species", sym("@@species")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "split", sym("@@split")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "toPrimitive", sym("@@toPrimitive")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "toStringTag", sym("@@toStringTag")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "unscopables", sym("@@unscopables")),

    (KILN_C99_BUILTIN_BUFFER, "Buffer", "from", code(KILN_C99_METHOD_BUFFER_FROM)),
    (KILN_C99_BUILTIN_BUFFER, "Buffer", "alloc", code(KILN_C99_METHOD_BUFFER_ALLOC)),
    // Three names for one operation: nothing here hands back uninitialised memory, so the
    // "unsafe" spellings are the safe one under another name.
    (KILN_C99_BUILTIN_BUFFER, "Buffer", "allocUnsafe", code(KILN_C99_METHOD_BUFFER_ALLOC)),
    (KILN_C99_BUILTIN_BUFFER, "Buffer", "allocUnsafeSlow", code(KILN_C99_METHOD_BUFFER_ALLOC)),
    (KILN_C99_BUILTIN_BUFFER, "Buffer", "isBuffer", code(KILN_C99_METHOD_BUFFER_IS_BUFFER)),
    (KILN_C99_BUILTIN_BUFFER, "Buffer", "byteLength", code(KILN_C99_METHOD_BUFFER_BYTE_LENGTH)),
    (KILN_C99_BUILTIN_BUFFER, "Buffer", "concat", code(KILN_C99_METHOD_BUFFER_CONCAT)),

    // Not tabulated by the interpreter, so the drift test reports it as unchecked rather than
    // passing it silently. The rows still make it self-describing.
    (KILN_C99_BUILTIN_ABORT_SIGNAL, "AbortSignal", "abort",
     code(KILN_C99_METHOD_ABORT_SIGNAL_ABORT)),
    (KILN_C99_BUILTIN_ABORT_SIGNAL, "AbortSignal", "timeout",
     code(KILN_C99_METHOD_ABORT_SIGNAL_TIMEOUT)),
    (KILN_C99_BUILTIN_ABORT_SIGNAL, "AbortSignal", "any", code(KILN_C99_METHOD_ABORT_SIGNAL_ANY)),

    (KILN_C99_BUILTIN_MATH, "Math", "abs", code(KILN_C99_METHOD_MATH_ABS)),
    (KILN_C99_BUILTIN_MATH, "Math", "floor", code(KILN_C99_METHOD_MATH_FLOOR)),
    (KILN_C99_BUILTIN_MATH, "Math", "ceil", code(KILN_C99_METHOD_MATH_CEIL)),
    (KILN_C99_BUILTIN_MATH, "Math", "round", code(KILN_C99_METHOD_MATH_ROUND)),
    (KILN_C99_BUILTIN_MATH, "Math", "trunc", code(KILN_C99_METHOD_MATH_TRUNC)),
    (KILN_C99_BUILTIN_MATH, "Math", "sign", code(KILN_C99_METHOD_MATH_SIGN)),
    (KILN_C99_BUILTIN_MATH, "Math", "sqrt", code(KILN_C99_METHOD_MATH_SQRT)),
    (KILN_C99_BUILTIN_MATH, "Math", "cbrt", code(KILN_C99_METHOD_MATH_CBRT)),
    (KILN_C99_BUILTIN_MATH, "Math", "random", code(KILN_C99_METHOD_MATH_RANDOM)),
    (KILN_C99_BUILTIN_MATH, "Math", "sinh", code(KILN_C99_METHOD_MATH_SINH)),
    (KILN_C99_BUILTIN_MATH, "Math", "cosh", code(KILN_C99_METHOD_MATH_COSH)),
    (KILN_C99_BUILTIN_MATH, "Math", "tanh", code(KILN_C99_METHOD_MATH_TANH)),
    (KILN_C99_BUILTIN_MATH, "Math", "asinh", code(KILN_C99_METHOD_MATH_ASINH)),
    (KILN_C99_BUILTIN_MATH, "Math", "acosh", code(KILN_C99_METHOD_MATH_ACOSH)),
    (KILN_C99_BUILTIN_MATH, "Math", "atanh", code(KILN_C99_METHOD_MATH_ATANH)),
    (KILN_C99_BUILTIN_MATH, "Math", "log1p", code(KILN_C99_METHOD_MATH_LOG1P)),
    (KILN_C99_BUILTIN_MATH, "Math", "expm1", code(KILN_C99_METHOD_MATH_EXPM1)),
    (KILN_C99_BUILTIN_MATH, "Math", "fround", code(KILN_C99_METHOD_MATH_FROUND)),
    (KILN_C99_BUILTIN_MATH, "Math", "clz32", code(KILN_C99_METHOD_MATH_CLZ32)),
    (KILN_C99_BUILTIN_MATH, "Math", "imul", code(KILN_C99_METHOD_MATH_IMUL)),
    (KILN_C99_BUILTIN_MATH, "Math", "hypot", code(KILN_C99_METHOD_MATH_HYPOT)),
    (KILN_C99_BUILTIN_MATH, "Math", "atan2", code(KILN_C99_METHOD_MATH_ATAN2)),
    (KILN_C99_BUILTIN_MATH, "Math", "sin", code(KILN_C99_METHOD_MATH_SIN)),
    (KILN_C99_BUILTIN_MATH, "Math", "cos", code(KILN_C99_METHOD_MATH_COS)),
    (KILN_C99_BUILTIN_MATH, "Math", "tan", code(KILN_C99_METHOD_MATH_TAN)),
    (KILN_C99_BUILTIN_MATH, "Math", "asin", code(KILN_C99_METHOD_MATH_ASIN)),
    (KILN_C99_BUILTIN_MATH, "Math", "acos", code(KILN_C99_METHOD_MATH_ACOS)),
    (KILN_C99_BUILTIN_MATH, "Math", "atan", code(KILN_C99_METHOD_MATH_ATAN)),
    (KILN_C99_BUILTIN_MATH, "Math", "pow", code(KILN_C99_METHOD_MATH_POW)),
    (KILN_C99_BUILTIN_MATH, "Math", "min", code(KILN_C99_METHOD_MATH_MIN)),
    (KILN_C99_BUILTIN_MATH, "Math", "max", code(KILN_C99_METHOD_MATH_MAX)),
    (KILN_C99_BUILTIN_MATH, "Math", "log", code(KILN_C99_METHOD_MATH_LOG)),
    (KILN_C99_BUILTIN_MATH, "Math", "log2", code(KILN_C99_METHOD_MATH_LOG2)),
    (KILN_C99_BUILTIN_MATH, "Math", "log10", code(KILN_C99_METHOD_MATH_LOG10)),
    (KILN_C99_BUILTIN_MATH, "Math", "exp", code(KILN_C99_METHOD_MATH_EXP)),
    (KILN_C99_BUILTIN_MATH, "Math", "PI", num("3.14159265358979311600")),
    (KILN_C99_BUILTIN_MATH, "Math", "E", num("2.71828182845904509080")),
    (KILN_C99_BUILTIN_MATH, "Math", "LN2", num("0.69314718055994530942")),
    (KILN_C99_BUILTIN_MATH, "Math", "LN10", num("2.30258509299404568402")),
    (KILN_C99_BUILTIN_MATH, "Math", "SQRT2", num("1.41421356237309504880")),
    (KILN_C99_BUILTIN_MATH, "Math", "SQRT1_2", num("0.70710678118654752440")),
    (KILN_C99_BUILTIN_MATH, "Math", "LOG2E", num("1.44269504088896340736")),
    (KILN_C99_BUILTIN_MATH, "Math", "LOG10E", num("0.43429448190325182765")),
    (KILN_C99_BUILTIN_MATH, "Math", "@@toStringTag", text("Math")),
}

/// A member the shared surface declares and this backend does not answer, with the reason.
///
/// The backlog is a list rather than a count on purpose. A gate that says "twenty-three gaps"
/// teaches people to ignore it; one that names them is a work queue, and each row is a sentence
/// somebody can act on or argue with. `builtin_surface_drift` in `kiln-native` prints them.
pub struct BuiltinGap {
    /// The global, spelled as [`kiln_builtin_surface::SURFACE`] spells it.
    pub global: &'static str,
    /// The member this backend does not answer.
    pub member: &'static str,
    /// Why it is still open. Never empty: a gap without a reason cannot be told from an oversight.
    pub why: &'static str,
}

/// Every member of the shared surface this backend does not implement.
///
/// Adding a member to `kiln_builtin_surface::SURFACE` is a compile error here until it appears
/// either in [`BUILTIN_SURFACE`] or in this list -- see [`assert_surface_is_covered`]. That is the
/// whole point: the decision to leave a member unimplemented becomes something somebody writes
/// down, rather than something a customer's bundle discovers.
/// The members this backend knows it does not answer, and why.
///
/// Declared through a macro for the same reason the surface is: the Rust list and the C table are
/// generated from one place, so they cannot disagree. The C side needs it because a built-in's
/// read path has to tell two things apart that look identical from inside the table -- a name the
/// language never had, which is `undefined`, and a member this backend has not built, which must
/// refuse by name. Without the backlog in C, answering `undefined` for an absent name would also
/// have answered it for `Object.groupBy`, and a silent wrong answer is worse than the refusal it
/// replaced.
macro_rules! builtin_backlog {
    ($(($global:literal, $member:literal, $why:literal)),* $(,)?) => {
        pub const BUILTIN_SURFACE_BACKLOG: &[BuiltinGap] = &[
            $(BuiltinGap { global: $global, member: $member, why: $why }),*
        ];
        macro_rules! emit_builtin_backlog_rows {
            () => { concat!($(concat!("    {\"", $global, "\", \"", $member, "\"},\n")),*) };
        }
    };
}

builtin_backlog! {
    ("Object", "groupBy",
     "not implemented; needs an iterator walk keyed by a callback's result"),
    ("Object", "seal",
     "sealing is weaker than freezing; answering freeze would be a silent wrong answer"),
    ("Object", "isSealed",
     "sealing is weaker than freezing; answering isFrozen would be a silent wrong answer"),
    ("Array", "fromAsync",
     "not implemented; needs the async-iterator protocol"),
    ("String", "fromCodePoint",
     "not implemented; builds a string from scalar values"),
    ("Promise", "try",
     "not implemented"),
    ("JSON", "rawJSON",
     "not implemented; the raw-JSON marker has no representation in this value kernel"),
    ("JSON", "isRawJSON",
     "not implemented; the raw-JSON marker has no representation in this value kernel"),
    ("Reflect", "defineProperty",
     "answers a boolean where Object.defineProperty answers the target, so it is not the \
              same operation under another name"),
    ("Reflect", "getPrototypeOf",
     "not implemented"),
    ("Reflect", "setPrototypeOf",
     "answers a boolean where Object.setPrototypeOf answers the target, so it is not the \
              same operation under another name"),
    ("Reflect", "isExtensible",
     "the object model carries extensibility now, but Reflect answers a boolean where \
              Object answers the target, and the two need separate codes"),
    ("Reflect", "preventExtensions",
     "the object model carries extensibility now, but Reflect answers a boolean where \
              Object answers the target, and the two need separate codes"),
    ("RegExp", "escape",
     "not implemented"),
    ("Error", "isError",
     "not implemented"),
    ("BigInt", "asIntN",
     "not implemented; needs a width-truncating call the BigInt provider does not export"),
    ("BigInt", "asUintN",
     "not implemented; needs a width-truncating call the BigInt provider does not export"),
    ("Map", "groupBy",
     "not implemented; needs an iterator walk keyed by a callback's result"),
    ("Iterator", "from",
     "refused by name: the Iterator helpers are in the unavailable-globals range"),
    ("Iterator", "concat",
     "refused by name: the Iterator helpers are in the unavailable-globals range"),
    ("Iterator", "zip",
     "refused by name: the Iterator helpers are in the unavailable-globals range"),
    ("Iterator", "zipKeyed",
     "refused by name: the Iterator helpers are in the unavailable-globals range"),
    ("Buffer", "constants",
     "not implemented; the constants namespace object has no representation here"),
    ("Math", "f16round",
     "not implemented; needs a half-precision round the number provider does not export"),
    ("%TypedArray%", "@@species",
     "not implemented; the intrinsic is not a value this backend hands back"),
    ("%TypedArrayConstructor%", "of",
     "answered from the receiver's element kind, not from a row"),
    ("%TypedArrayConstructor%", "from",
     "answered from the receiver's element kind, not from a row"),
    ("%TypedArrayConstructor%", "BYTES_PER_ELEMENT",
     "answered from the receiver's element kind, not from a row"),
    ("Uint8Array", "of",
     "answered from the receiver's element kind, not from a row"),
    ("Uint8Array", "from",
     "answered from the receiver's element kind, not from a row"),
    ("Uint8Array", "BYTES_PER_ELEMENT",
     "answered from the receiver's element kind, not from a row"),
    ("Uint8Array", "fromBase64",
     "not implemented"),
    ("Uint8Array", "fromHex",
     "not implemented"),
    ("Atomics", "add",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "and",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "compareExchange",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "exchange",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "isLockFree",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "load",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "notify",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "or",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "pause",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "store",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "sub",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "wait",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "waitAsync",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "xor",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "@@toStringTag",
     "refused by name: shared memory has no representation here"),
}

const fn surface_answers(surface: &[BuiltinMember], global: &str, member: &str) -> bool {
    let mut index = 0;
    while index < surface.len() {
        if method_name_is(surface[index].owner, global)
            && method_name_is(surface[index].name, member)
        {
            return true;
        }
        index += 1;
    }
    false
}

const fn backlog_names(backlog: &[BuiltinGap], global: &str, member: &str) -> bool {
    let mut index = 0;
    while index < backlog.len() {
        if method_name_is(backlog[index].global, global)
            && method_name_is(backlog[index].member, member)
        {
            return true;
        }
        index += 1;
    }
    false
}

/// Every member of the shared surface is either implemented here or written down as backlog, and
/// nothing here claims a member the shared surface does not declare.
///
/// THIS IS THE GATE, AND IT IS A COMPILE ERROR RATHER THAN A TEST FAILURE. A member added to
/// `kiln_builtin_surface::SURFACE` -- which is also where the interpreter reads its own lists --
/// does not build here until somebody decides what this backend does about it. That is the
/// difference between drift being structurally impossible and merely untested, which is the whole
/// of ADR 0043.
///
/// A `const` panic carries a literal and nothing else, so this says only *that* the two disagree.
/// `builtin_surface_drift` in `kiln-native` reads the same three tables and says which members,
/// with each backlog row's reason. The compiler is the gate; the test is the explanation.
const fn assert_surface_is_covered(surface: &[BuiltinMember], backlog: &[BuiltinGap]) {
    let shared = kiln_builtin_surface::SURFACE;
    let mut index = 0;
    while index < shared.len() {
        let members = shared[index].members;
        let mut member = 0;
        while member < members.len() {
            assert!(
                surface_answers(surface, shared[index].global, members[member])
                    || backlog_names(backlog, shared[index].global, members[member]),
                "a member of kiln_builtin_surface::SURFACE is neither in BUILTIN_SURFACE nor in \
                 BUILTIN_SURFACE_BACKLOG; run `mbx test -p kiln-native --test \
                 builtin_surface_drift` to be told which one"
            );
            member += 1;
        }
        index += 1;
    }

    // The other direction. A member only one engine has is drift whichever engine has it, and
    // this backend inventing one is the harder half to notice: nothing refuses, and the two
    // engines simply answer differently.
    index = 0;
    while index < surface.len() {
        assert!(
            !kiln_builtin_surface::is_declared(surface[index].owner)
                || kiln_builtin_surface::has(surface[index].owner, surface[index].name),
            "BUILTIN_SURFACE answers a member kiln_builtin_surface::SURFACE does not declare; \
             either declare it there so both engines have it, or drop the row"
        );
        index += 1;
    }

    // A backlog row that is no longer true is worse than no backlog: it describes a past that has
    // been fixed, and the next reader believes it.
    index = 0;
    while index < backlog.len() {
        assert!(
            !backlog[index].why.is_empty(),
            "a BUILTIN_SURFACE_BACKLOG row has no reason, which cannot be told from an oversight"
        );
        assert!(
            kiln_builtin_surface::is_declared(backlog[index].global)
                && kiln_builtin_surface::has(backlog[index].global, backlog[index].member),
            "a BUILTIN_SURFACE_BACKLOG row names a member the shared surface does not declare"
        );
        assert!(
            !surface_answers(surface, backlog[index].global, backlog[index].member),
            "a BUILTIN_SURFACE_BACKLOG row names a member BUILTIN_SURFACE now answers; delete the \
             backlog row"
        );
        index += 1;
    }
}

const _: () = assert_surface_is_covered(BUILTIN_SURFACE, BUILTIN_SURFACE_BACKLOG);

/// The globals [`BUILTIN_SURFACE`] speaks for, in declaration order and without repeats.
pub fn builtin_surface_owners() -> Vec<&'static str> {
    let mut owners: Vec<&'static str> = Vec::new();
    for member in BUILTIN_SURFACE {
        if !owners.contains(&member.owner) {
            owners.push(member.owner);
        }
    }
    owners
}

/// Every member name `global` answers to natively, in declaration order.
pub fn builtin_surface_members(global: &str) -> Vec<&'static str> {
    BUILTIN_SURFACE
        .iter()
        .filter(|member| member.owner == global)
        .map(|member| member.name)
        .collect()
}

const HEADER: &str = concat!(
    r#"/* Kiln C99 backend: deterministic scalar unit. */
#include <ctype.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* C ABI version 1; HostOps ABI version 1. */
#define KILN_NUMBER_FORMAT_VERSION 1
#define KILN_NUMBER_PARSE_VERSION 1
#define KILN_C99_OK 0
#define KILN_C99_INVALID_ARGUMENT (-1)
#define KILN_C99_CONTROL_ERROR (-2)
#define KILN_C99_HOST_ERROR (-3)
#define KILN_C99_OUT_OF_MEMORY (-4)
#define KILN_C99_CALL_DEPTH_ERROR (-5)
#define KILN_C99_UNINITIALIZED_ERROR (-6)
/* A value the program threw, travelling outward until a handler takes it.  Distinct from every
 * other status because those mean "this cannot be done" and this one means "the program said
 * so": a `catch` takes this and must not take the others. */
#define KILN_C99_THROWN (-7)

/* Refusals are runtime/compiler diagnostics, not JavaScript exceptions.  Keep the most specific
 * detail at the failing helper and attach the generated module + bytecode site as control
 * unwinds.  The entry boundary renders this into its ordinary scalar result, so no private
 * runtime pointer or new public ABI type escapes. */
static const char *kiln_c99_diagnostic_site = NULL;
static const char *kiln_c99_diagnostic_operation = NULL;
static size_t kiln_c99_diagnostic_instruction = 0;
static size_t kiln_c99_diagnostic_depth = 0;
static uint64_t kiln_c99_diagnostic_method = 0;
static uint64_t kiln_c99_diagnostic_receiver_tag = 0;
static int kiln_c99_diagnostic_has_method = 0;
static const char *kiln_c99_diagnostic_detail = NULL;
static const char *kiln_c99_diagnostic_function = NULL;
static size_t kiln_c99_diagnostic_function_len = 0;
static int kiln_c99_diagnostic_active_depth = 0;

/* Recursion runs on the C stack, where exhausting it aborts the process instead of reporting
 * anything.  The interpreter's own limit is 10000 frames, but its frames are heap-allocated and
 * these are not: a generated body holds its stack and locals as automatic arrays, so the safe
 * bound is far lower and has to be enforced rather than hoped for. */
#define KILN_C99_MAX_CALL_DEPTH 256

enum kiln_value_tag {
    KILN_VALUE_UNDEFINED = 0,
    KILN_VALUE_NULL = 1,
    KILN_VALUE_BOOLEAN = 2,
    KILN_VALUE_NUMBER = 3,
    KILN_VALUE_STRING = 4,
    /* The two tags below are internal to a generated unit and never cross the embedding
     * boundary: a function is converted to text before any host call and before the module
     * result, and a cell is never a program-visible value at all.  That is what keeps
     * KILN_C_ABI_VERSION at 1 -- the boundary value is still exactly tags 0 through 4. */

    /* bits holds a kiln_closure*. */
    KILN_VALUE_FUNCTION = 5,
    /* A shared box; bits holds a kiln_value*.  It appears only in a local slot the compiler
     * marked captured, and in a closure's upvalue array. */
    KILN_VALUE_CELL = 6,
    /* A `let` or `const` slot before its declaration runs.  Reading one is the temporal dead
     * zone, and it is a distinct state from undefined: `let x; x` is undefined, `x; let x` is
     * an error.  Only slots the program actually marks are ever tested for it. */
    KILN_VALUE_UNINITIALIZED = 7,
    /* A callable the runtime supplies rather than the program.  bits holds the builtin's code
     * below.  It is a separate tag from a closure because it has no body to point at and no
     * captures to carry. */
    KILN_VALUE_BUILTIN = 8,
    /* bits holds a kiln_object*.  Objects and arrays are separate tags because `typeof` and the
     * two index paths differ; they share nothing but the tag space. */
    KILN_VALUE_OBJECT = 9,
    KILN_VALUE_ARRAY = 10,
    /* A method the runtime supplies.  bits holds its code, and *not* its receiver: `a.map` and
     * `b.map` are the same function object in the language, which is what makes
     * `a.map === b.map` true, and `CallMethod` keeps the receiver on the stack anyway. */
    KILN_VALUE_METHOD = 11,
    /* bits holds a kiln_generator*: a body that has been started and not finished. */
    KILN_VALUE_GENERATOR = 12,
    /* bits holds a double* -- an instant in milliseconds since the epoch. A pointer rather than
     * the value itself because a Date is mutable: `setFullYear` writes through it. */
    KILN_VALUE_DATE = 13,
    /* An array slot that was never written, or was deleted.
     *
     * It *reads* as undefined, which is why the marker must never escape into an expression --
     * but it is not one: `1 in [1, , 3]` is false where `1 in [1, undefined, 3]` is true, a
     * console shows `<1 empty item>`, and every element-walking method skips it. */
    KILN_VALUE_HOLE = 14,
    /* bits holds a kiln_collection*. A Map and a Set are one tag because they are one structure:
     * a Set is a Map whose values are never read. */
    KILN_VALUE_COLLECTION = 15,
    /* bits holds a kiln_promise*: a value that arrives once, later. */
    KILN_VALUE_PROMISE = 16,
    /* The `resolve` and `reject` an executor is handed. bits holds a kiln_settler*. */
    KILN_VALUE_SETTLER = 17,
    /* A callable with its receiver fixed. bits holds a kiln_bound*.
     *
     * Needed because a promise reaction is a handler *and* the thing it runs against, and the
     * queue can only hold one value per slot -- so the pair has to be one value. */
    KILN_VALUE_BOUND = 18,
    /* A property key with an identity of its own. bits holds a kiln_symbol*. */
    KILN_VALUE_SYMBOL = 19,
    /* A compiled pattern and its cursor. bits holds a kiln_regexp*. */
    KILN_VALUE_REGEXP = 20,
    /* An arbitrary-precision integer. bits holds the provider's opaque handle. */
    KILN_VALUE_BIGINT = 21,
    /* Typed views are private heap values.  Their pointers never cross the scalar embedding ABI. */
    KILN_VALUE_ARRAY_BUFFER = 22,
    KILN_VALUE_TYPED_ARRAY = 23,
    /* A target with a handler standing in front of it. bits holds a kiln_proxy*.
     *
     * A tag of its own rather than a flag on an object, because a proxy is not an object with
     * extra behaviour: it can stand in front of an array or a function just as well, `typeof`
     * asks the thing behind it, and every property operation on it is a *call*. Nothing that
     * tests for one of the tags above may accidentally match it. */
    KILN_VALUE_PROXY = 24,
    /* One of the eight `Temporal` types. bits holds the provider's opaque handle.
     *
     * A tag rather than an object because every one of them is immutable and every operation on
     * one has to reach the engine anyway (ADR 0025): an object would be a second place for the
     * state to live and a second thing to keep in step. */
    KILN_VALUE_TEMPORAL = 25,
    /* A graph-scoped CommonJS require function. Private to native graph objects: bits holds a
     * kiln_commonjs_require*, whose callback and opaque context are supplied by the launcher. */
    KILN_VALUE_COMMONJS_REQUIRE = 26
};

#define KILN_C99_BUILTIN_PROMISE 15
#define KILN_C99_BUILTIN_BOOLEAN 16
#define KILN_C99_BUILTIN_REFLECT 17
/* `Object.prototype`, which a program reaches for its methods rather than as an object:
 * `Object.prototype.hasOwnProperty.call(x, k)` is how every transpiled module asks whether a key
 * is own. It answers the three names it carries and nothing else -- it is not yet the object the
 * chain actually ends at, which is what `Object.getPrototypeOf` would need. */
#define KILN_C99_BUILTIN_OBJECT_PROTOTYPE 18
/* ...and the same for the two whose methods a program borrows most:
 * `Array.prototype.slice.call(arguments)` is how a transpiled module makes an array. */
#define KILN_C99_BUILTIN_ARRAY_PROTOTYPE 19
#define KILN_C99_BUILTIN_STRING_PROTOTYPE 20
#define KILN_C99_BUILTIN_SYMBOL 21
#define KILN_C99_BUILTIN_REGEXP 22
/* The rest of the prototype objects a program borrows a method from.
 * `RegExp.prototype.test.call(re, s)` is how a library calls one without trusting the receiver. */
#define KILN_C99_BUILTIN_REGEXP_PROTOTYPE 23
#define KILN_C99_BUILTIN_DATE_PROTOTYPE 24
#define KILN_C99_BUILTIN_NUMBER_PROTOTYPE 25
#define KILN_C99_BUILTIN_FUNCTION_PROTOTYPE 26
/* `Function`, reached for its `prototype` rather than to construct. */
#define KILN_C99_BUILTIN_FUNCTION 27
/* `BigInt`, which converts and never constructs: `new BigInt(1)` is a TypeError in the
 * specification, and the reason is that a BigInt is a primitive. */
#define KILN_C99_BUILTIN_BIGINT 28
#define KILN_C99_BUILTIN_ARRAY_BUFFER 29
#define KILN_C99_BUILTIN_INT8_ARRAY 30
#define KILN_C99_BUILTIN_UINT8_ARRAY 31
#define KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY 32
#define KILN_C99_BUILTIN_INT16_ARRAY 33
#define KILN_C99_BUILTIN_UINT16_ARRAY 34
#define KILN_C99_BUILTIN_INT32_ARRAY 35
#define KILN_C99_BUILTIN_UINT32_ARRAY 36
#define KILN_C99_BUILTIN_FLOAT32_ARRAY 37
#define KILN_C99_BUILTIN_FLOAT64_ARRAY 38
/* Private prototype markers for the numeric view constructors. Concrete prototypes own their
 * constructor metadata and all inherit one common typed-array method table. */
#define KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE 39
#define KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE 40
#define KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE 41
#define KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY_PROTOTYPE 42
#define KILN_C99_BUILTIN_INT16_ARRAY_PROTOTYPE 43
#define KILN_C99_BUILTIN_UINT16_ARRAY_PROTOTYPE 44
#define KILN_C99_BUILTIN_INT32_ARRAY_PROTOTYPE 45
#define KILN_C99_BUILTIN_UINT32_ARRAY_PROTOTYPE 46
#define KILN_C99_BUILTIN_FLOAT32_ARRAY_PROTOTYPE 47
#define KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE 48

/* The two collection prototype tables. They exist for the same reason the others do: a class
 * that extends `Map` needs `Map.prototype` to link its own prototype to, and without one the
 * whole of `class Registry extends Map {}` is unreachable. */
#define KILN_C99_BUILTIN_MAP_PROTOTYPE 49
#define KILN_C99_BUILTIN_SET_PROTOTYPE 50

/* One table for every error kind rather than one each. The chain is not what answers a question
 * about an error here -- `instanceof` reads the kind the instance records, and `name`, `message`
 * and `stack` are own properties of the instance -- so the table exists to be a *link*, which is
 * what `class HttpError extends Error {}` needs and all it needs. */
#define KILN_C99_BUILTIN_ERROR_PROTOTYPE 51

/* `Proxy` is a constructor and not a namespace: `typeof Proxy` is "function", and the only way
 * to reach one is `new`. */
#define KILN_C99_BUILTIN_PROXY 52

/* `Temporal`: the namespace, its eight types, and `Temporal.Now`.
 *
 * The eight are contiguous and in the provider's own kind order, so the code the provider takes is
 * `builtin - KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE` rather than a translation table that could
 * drift away from it. */
#define KILN_C99_BUILTIN_TEMPORAL 53
#define KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE 54
#define KILN_C99_BUILTIN_TEMPORAL_PLAIN_TIME 55
#define KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE_TIME 56
#define KILN_C99_BUILTIN_TEMPORAL_PLAIN_YEAR_MONTH 57
#define KILN_C99_BUILTIN_TEMPORAL_PLAIN_MONTH_DAY 58
#define KILN_C99_BUILTIN_TEMPORAL_DURATION 59
#define KILN_C99_BUILTIN_TEMPORAL_INSTANT 60
#define KILN_C99_BUILTIN_TEMPORAL_ZONED_DATE_TIME 61
#define KILN_C99_BUILTIN_TEMPORAL_NOW 62
/* WHATWG URL. The object it constructs is ordinary; only construction and query lookup need
 * runtime code, so no URL-specific value tag or ABI representation crosses the boundary. */
#define KILN_C99_BUILTIN_URL 63
#define KILN_C99_BUILTIN_BUFFER 64

/* Array methods, part two.
 *
 * Numbered here rather than beside the first eleven because the block that answers a *namespace*
 * method uses the generated dispatch category, and everything reached through an array receiver
 * has to stay outside it -- as is the block that answers a *string* method. The gap is deliberate:
 * none of these codes reaches a cache key, but renumbering the block would still be a diff nobody
 * could review. */
"#,
    emit_method_define!(KILN_C99_METHOD_ARRAY_AT),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FIND),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FIND_INDEX),
    emit_method_define!(KILN_C99_METHOD_ARRAY_SOME),
    emit_method_define!(KILN_C99_METHOD_ARRAY_EVERY),
    emit_method_define!(KILN_C99_METHOD_ARRAY_SORT),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FLAT),
    emit_method_define!(KILN_C99_METHOD_ARRAY_ENTRIES),
    emit_method_define!(KILN_C99_METHOD_ARRAY_KEYS),
    emit_method_define!(KILN_C99_METHOD_ARRAY_REVERSE),
    emit_method_define!(KILN_C99_METHOD_ARRAY_SHIFT),
    emit_method_define!(KILN_C99_METHOD_ARRAY_UNSHIFT),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FILL),
    emit_method_define!(KILN_C99_METHOD_ARRAY_LAST_INDEX_OF),
    emit_method_define!(KILN_C99_METHOD_ARRAY_SPLICE),
    emit_method_define!(KILN_C99_METHOD_ARRAY_COPY_WITHIN),
    emit_method_define!(KILN_C99_METHOD_ARRAY_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FLAT_MAP),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FIND_LAST),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX),
    emit_method_define!(KILN_C99_METHOD_ARRAY_REDUCE_RIGHT),
    r#"
/* String methods, part two, numbered above the array block for the same reason the array block is
 * numbered above the namespace one: the guard that routes a code to a receiver kind is a range,
 * and a new range is cheaper than renumbering an old one. */
"#,
    emit_method_define!(KILN_C99_METHOD_STRING_CODE_POINT_AT),
    emit_method_define!(KILN_C99_METHOD_STRING_SUBSTR),
    emit_method_define!(KILN_C99_METHOD_STRING_LOCALE_COMPARE),
    emit_method_define!(KILN_C99_METHOD_STRING_NORMALIZE),
    emit_method_define!(KILN_C99_METHOD_STRING_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_STRING_VALUE_OF),
    r#"/* These three only ever take a pattern, so they are intercepted before the string block: they
 * need to call back into the program, and the string block has no interpreter to call with. */
"#,
    emit_method_define!(KILN_C99_METHOD_STRING_MATCH),
    emit_method_define!(KILN_C99_METHOD_STRING_MATCH_ALL),
    emit_method_define!(KILN_C99_METHOD_STRING_SEARCH),
    r#"
/* `Error.prototype.toString`, which every error inherits and no other object does. Deliberately
 * past the end of every range `kiln_c99_lookup_method` searches: it is not reached by name from a
 * table but handed out by the object read that recognises an error, because an ordinary object
 * with a `toString` of its own must not find this one. */
"#,
    emit_method_define!(KILN_C99_METHOD_ERROR_TO_STRING),
    r#"
#define KILN_C99_BUILTIN_MAP 8
#define KILN_C99_BUILTIN_SET 9
/* The error constructors, contiguous so the kind is the offset: `new TypeError(m)` and
 * `new Error(m)` differ only in the name they write. */
#define KILN_C99_BUILTIN_ERROR 10
#define KILN_C99_BUILTIN_TYPE_ERROR 11
#define KILN_C99_BUILTIN_RANGE_ERROR 12
#define KILN_C99_BUILTIN_REFERENCE_ERROR 13
#define KILN_C99_BUILTIN_SYNTAX_ERROR 14
#define KILN_C99_BUILTIN_URI_ERROR 65
#define KILN_C99_BUILTIN_WEAK_MAP 66
#define KILN_C99_BUILTIN_WEAK_SET 67
#define KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE 68
#define KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE 69
#define KILN_C99_BUILTIN_PROMISE_PROTOTYPE 70
/* The error kind that reports several failures as one. It has its own value (rather than sharing
 * the promise prototype's marker) because the kind is stored in every error instance. */
#define KILN_C99_BUILTIN_AGGREGATE_ERROR 74
#define KILN_C99_BUILTIN_ABORT_CONTROLLER 71
#define KILN_C99_BUILTIN_ABORT_SIGNAL 72
#define KILN_C99_BUILTIN_TEXT_ENCODER 73
#define KILN_C99_BUILTIN_AGGREGATE_ERROR_PROTOTYPE 75

/* The globals this backend does not implement yet, published anyway so that `typeof` tells the
 * truth about them.
 *
 * WHY A VALUE AT ALL, RATHER THAN LEAVING THE NAME ABSENT. `typeof X === "undefined"` is how real
 * code feature-detects, so a name the interpreter answers `"function"` for and this backend
 * answers `"undefined"` for does not make a compiled program fail -- it makes it silently take
 * the *other* branch. That is a wrong answer, and a wrong answer is the one thing this backend is
 * not allowed to give. A refusal is honest: the program stops and the message names the operation
 * nobody implemented. So each of these carries the right `typeof` and the right `name`, and every
 * other use of one reaches the refusal at the end of the builtin read, or the refusal at the end
 * of the construct dispatch, and says so.
 *
 * Contiguous, and bounded by the two markers, because the only thing the rest of the file needs
 * to ask about them is "is this one of them" -- `kiln_c99_is_unavailable_global`. */
#define KILN_C99_BUILTIN_UNAVAILABLE_FIRST 76
#define KILN_C99_BUILTIN_INTL 76
#define KILN_C99_BUILTIN_DATA_VIEW 77
#define KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER 78
#define KILN_C99_BUILTIN_WEAK_REF 79
#define KILN_C99_BUILTIN_FINALIZATION_REGISTRY 80
#define KILN_C99_BUILTIN_SHADOW_REALM 81
#define KILN_C99_BUILTIN_ITERATOR 82
#define KILN_C99_BUILTIN_FLOAT16_ARRAY 83
#define KILN_C99_BUILTIN_BIG_INT64_ARRAY 84
#define KILN_C99_BUILTIN_BIG_UINT64_ARRAY 85
#define KILN_C99_BUILTIN_DISPOSABLE_STACK 86
#define KILN_C99_BUILTIN_ASYNC_DISPOSABLE_STACK 87
#define KILN_C99_BUILTIN_SUPPRESSED_ERROR 88
#define KILN_C99_BUILTIN_EVAL_ERROR 89
#define KILN_C99_BUILTIN_TEXT_DECODER 90
#define KILN_C99_BUILTIN_EVAL 91
#define KILN_C99_BUILTIN_PERFORMANCE 92
#define KILN_C99_BUILTIN_UNAVAILABLE_LAST 92

#define KILN_C99_BUILTIN_DATE 7

/* A body that yielded rather than returned.  Not an error: the caller is the generator's `next`,
 * which turns it into `{ value, done: false }`. */
#define KILN_C99_YIELDED 1

/* `String(x)`.  A template's `${x}` lowers to a call to it rather than to `+`, because the two
 * use different hints: `${x}` asks `toString` first and `+` asks `valueOf` first. */
#define KILN_C99_BUILTIN_STRING 1
#define KILN_C99_BUILTIN_MATH 2
#define KILN_C99_BUILTIN_NUMBER 3
#define KILN_C99_BUILTIN_OBJECT 4
#define KILN_C99_BUILTIN_ARRAY 5
#define KILN_C99_BUILTIN_JSON 6

/* Array methods.  Stable within a generated unit only -- unlike the instruction and constant
 * tags, none of these reaches a cache key. */
"#,
    emit_method_define!(KILN_C99_METHOD_PUSH),
    emit_method_define!(KILN_C99_METHOD_POP),
    emit_method_define!(KILN_C99_METHOD_JOIN),
    emit_method_define!(KILN_C99_METHOD_INDEX_OF),
    emit_method_define!(KILN_C99_METHOD_INCLUDES),
    emit_method_define!(KILN_C99_METHOD_SLICE),
    emit_method_define!(KILN_C99_METHOD_CONCAT),
    emit_method_define!(KILN_C99_METHOD_MAP),
    emit_method_define!(KILN_C99_METHOD_FILTER),
    emit_method_define!(KILN_C99_METHOD_FOR_EACH),
    emit_method_define!(KILN_C99_METHOD_REDUCE),
    r#"
/* String methods.  `slice` and `indexOf` are shared with the array table by name and told apart
 * by the receiver, exactly as the language does. */
"#,
    emit_method_define!(KILN_C99_METHOD_STRING_SLICE),
    emit_method_define!(KILN_C99_METHOD_SUBSTRING),
    emit_method_define!(KILN_C99_METHOD_STRING_INDEX_OF),
    emit_method_define!(KILN_C99_METHOD_LAST_INDEX_OF),
    emit_method_define!(KILN_C99_METHOD_STRING_INCLUDES),
    emit_method_define!(KILN_C99_METHOD_STARTS_WITH),
    emit_method_define!(KILN_C99_METHOD_ENDS_WITH),
    emit_method_define!(KILN_C99_METHOD_CHAR_AT),
    emit_method_define!(KILN_C99_METHOD_CHAR_CODE_AT),
    emit_method_define!(KILN_C99_METHOD_STRING_AT),
    emit_method_define!(KILN_C99_METHOD_TRIM),
    emit_method_define!(KILN_C99_METHOD_TRIM_START),
    emit_method_define!(KILN_C99_METHOD_TRIM_END),
    emit_method_define!(KILN_C99_METHOD_SPLIT),
    emit_method_define!(KILN_C99_METHOD_REPEAT),
    emit_method_define!(KILN_C99_METHOD_PAD_START),
    emit_method_define!(KILN_C99_METHOD_PAD_END),
    emit_method_define!(KILN_C99_METHOD_TO_UPPER_CASE),
    emit_method_define!(KILN_C99_METHOD_TO_LOWER_CASE),
    emit_method_define!(KILN_C99_METHOD_REPLACE),
    emit_method_define!(KILN_C99_METHOD_REPLACE_ALL),
    emit_method_define!(KILN_C99_METHOD_STRING_CONCAT),
    r#"
/* Namespace methods.  Unlike the array and string tables these take no receiver: `Math.floor` is
 * reached through `Math`, but `Math` contributes nothing to the answer. */
"#,
    emit_method_define!(KILN_C99_METHOD_MATH_FLOOR),
    emit_method_define!(KILN_C99_METHOD_MATH_CEIL),
    emit_method_define!(KILN_C99_METHOD_MATH_ROUND),
    emit_method_define!(KILN_C99_METHOD_MATH_TRUNC),
    emit_method_define!(KILN_C99_METHOD_MATH_ABS),
    emit_method_define!(KILN_C99_METHOD_MATH_SIGN),
    emit_method_define!(KILN_C99_METHOD_MATH_SQRT),
    emit_method_define!(KILN_C99_METHOD_MATH_CBRT),
    emit_method_define!(KILN_C99_METHOD_MATH_POW),
    emit_method_define!(KILN_C99_METHOD_MATH_MIN),
    emit_method_define!(KILN_C99_METHOD_MATH_MAX),
    emit_method_define!(KILN_C99_METHOD_MATH_LOG),
    emit_method_define!(KILN_C99_METHOD_MATH_LOG2),
    emit_method_define!(KILN_C99_METHOD_MATH_LOG10),
    emit_method_define!(KILN_C99_METHOD_MATH_EXP),
    emit_method_define!(KILN_C99_METHOD_MATH_HYPOT),
    emit_method_define!(KILN_C99_METHOD_MATH_ATAN2),
    emit_method_define!(KILN_C99_METHOD_MATH_SIN),
    emit_method_define!(KILN_C99_METHOD_MATH_COS),
    emit_method_define!(KILN_C99_METHOD_MATH_TAN),
    emit_method_define!(KILN_C99_METHOD_MATH_ASIN),
    emit_method_define!(KILN_C99_METHOD_MATH_ACOS),
    emit_method_define!(KILN_C99_METHOD_MATH_ATAN),
    emit_method_define!(KILN_C99_METHOD_IS_INTEGER),
    emit_method_define!(KILN_C99_METHOD_IS_SAFE_INTEGER),
    emit_method_define!(KILN_C99_METHOD_IS_NAN),
    emit_method_define!(KILN_C99_METHOD_IS_FINITE),
    emit_method_define!(KILN_C99_METHOD_OBJECT_KEYS),
    emit_method_define!(KILN_C99_METHOD_OBJECT_VALUES),
    emit_method_define!(KILN_C99_METHOD_OBJECT_ENTRIES),
    emit_method_define!(KILN_C99_METHOD_OBJECT_ASSIGN),
    emit_method_define!(KILN_C99_METHOD_OBJECT_PREVENT_EXTENSIONS),
    emit_method_define!(KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE),
    emit_method_define!(KILN_C99_METHOD_HAS_OWN),
    emit_method_define!(KILN_C99_METHOD_IS_ARRAY),
    emit_method_define!(KILN_C99_METHOD_ARRAY_OF),
    emit_method_define!(KILN_C99_METHOD_OBJECT_CREATE),
    emit_method_define!(KILN_C99_METHOD_JSON_STRINGIFY),
    emit_method_define!(KILN_C99_METHOD_JSON_PARSE),
    emit_method_define!(KILN_C99_METHOD_GENERATOR_NEXT),
    emit_method_define!(KILN_C99_METHOD_GENERATOR_RETURN),
    emit_method_define!(KILN_C99_METHOD_GENERATOR_SELF),
    r#"
/* Date readers. Every one of these is UTC, because the realm has one zone and it is UTC
 * (ADR 0015) -- so the local reader and the UTC reader are the same function rather than two
 * that happen to agree. */
"#,
    emit_method_define!(KILN_C99_METHOD_GET_TIME),
    emit_method_define!(KILN_C99_METHOD_GET_FULL_YEAR),
    emit_method_define!(KILN_C99_METHOD_GET_MONTH),
    emit_method_define!(KILN_C99_METHOD_GET_DATE),
    emit_method_define!(KILN_C99_METHOD_GET_DAY),
    emit_method_define!(KILN_C99_METHOD_GET_HOURS),
    emit_method_define!(KILN_C99_METHOD_GET_MINUTES),
    emit_method_define!(KILN_C99_METHOD_GET_SECONDS),
    emit_method_define!(KILN_C99_METHOD_GET_MILLISECONDS),
    emit_method_define!(KILN_C99_METHOD_GET_TIMEZONE_OFFSET),
    emit_method_define!(KILN_C99_METHOD_TO_ISO_STRING),
    emit_method_define!(KILN_C99_METHOD_TO_JSON),
    emit_method_define!(KILN_C99_METHOD_DATE_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_DATE_NOW),
    emit_method_define!(KILN_C99_METHOD_DATE_UTC),
    emit_method_define!(KILN_C99_METHOD_DATE_PARSE),
    r#"
/* The end of every chain is `Object.prototype`, and these are what it carries. */
"#,
    emit_method_define!(KILN_C99_METHOD_HAS_OWN_PROPERTY),
    emit_method_define!(KILN_C99_METHOD_OBJECT_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING),
    emit_method_define!(KILN_C99_METHOD_OBJECT_VALUE_OF),
    r#"
/* Map and Set. `size` is a property rather than a method, so it is answered by the read. */
"#,
    emit_method_define!(KILN_C99_METHOD_COLLECTION_GET),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_SET),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_ADD),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_HAS),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_DELETE),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_CLEAR),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_FOR_EACH),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_KEYS),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_VALUES),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_ENTRIES),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_SELF),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_NEXT),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_SELF),
    emit_method_define!(KILN_C99_METHOD_ARRAY_VALUES),
    emit_method_define!(KILN_C99_METHOD_THROW_TYPE_ERROR),
    r#"
/* Promise, and the two halves of driving an async function. */
"#,
    emit_method_define!(KILN_C99_METHOD_THEN),
    emit_method_define!(KILN_C99_METHOD_CATCH),
    emit_method_define!(KILN_C99_METHOD_FINALLY),
    emit_method_define!(KILN_C99_METHOD_PROMISE_RESOLVE),
    emit_method_define!(KILN_C99_METHOD_PROMISE_REJECT),
    emit_method_define!(KILN_C99_METHOD_PROMISE_ALL),
    emit_method_define!(KILN_C99_METHOD_PROMISE_RACE),
    emit_method_define!(KILN_C99_METHOD_ASYNC_RESUME),
    emit_method_define!(KILN_C99_METHOD_ASYNC_THROW),
    emit_method_define!(KILN_C99_METHOD_ALL_STEP),
    r#"
/* The global functions. They are namespace methods with no namespace: nothing is reached through
 * before them, so `LoadBuiltin` pushes the method value directly. Two of them share a name with a
 * `Number` method and not its behaviour -- `isNaN('x')` coerces where `Number.isNaN('x')` does
 * not -- so they are separate codes rather than the same one reached two ways. */
"#,
    emit_method_define!(KILN_C99_METHOD_PARSE_INT),
    emit_method_define!(KILN_C99_METHOD_PARSE_FLOAT),
    emit_method_define!(KILN_C99_METHOD_GLOBAL_IS_NAN),
    emit_method_define!(KILN_C99_METHOD_GLOBAL_IS_FINITE),
    r#"
/* Number methods. Read off the value rather than through a `Number.prototype` object, which is
 * what the interpreter does and for the same reason: there is no such object yet. */
"#,
    emit_method_define!(KILN_C99_METHOD_NUMBER_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_NUMBER_TO_FIXED),
    emit_method_define!(KILN_C99_METHOD_NUMBER_TO_EXPONENTIAL),
    emit_method_define!(KILN_C99_METHOD_NUMBER_TO_PRECISION),
    emit_method_define!(KILN_C99_METHOD_NUMBER_VALUE_OF),
    r#"
/* The text-escaping globals. `escape`/`unescape` are Annex B and work in code units; the four URI
 * functions work in UTF-8 bytes and differ only in which characters they leave alone. */
"#,
    emit_method_define!(KILN_C99_METHOD_ESCAPE),
    emit_method_define!(KILN_C99_METHOD_UNESCAPE),
    emit_method_define!(KILN_C99_METHOD_ENCODE_URI),
    emit_method_define!(KILN_C99_METHOD_ENCODE_URI_COMPONENT),
    emit_method_define!(KILN_C99_METHOD_DECODE_URI),
    emit_method_define!(KILN_C99_METHOD_DECODE_URI_COMPONENT),
    r#"
/* `queueMicrotask(fn)`: the queue promises already drain, reached directly. */
"#,
    emit_method_define!(KILN_C99_METHOD_QUEUE_MICROTASK),
    r#"
/* The timer functions. `clearTimeout` and `clearInterval` do the same thing and are two codes
 * because they are two names: a program can print either one. */
"#,
    emit_method_define!(KILN_C99_METHOD_SET_TIMEOUT),
    emit_method_define!(KILN_C99_METHOD_SET_INTERVAL),
    emit_method_define!(KILN_C99_METHOD_CLEAR_TIMEOUT),
    emit_method_define!(KILN_C99_METHOD_CLEAR_INTERVAL),
    r#"
/* The two combinators that do not fail on the first rejection. Named outside the contiguous
 * `Promise` range because that range is how a namespace's methods are found, and it was already
 * closed. `@@rejectStep` is the rejection-side twin of `@@allStep`: `all` and `race` settle their
 * promise on any rejection and need no handler of their own, and these two do. */
"#,
    emit_method_define!(KILN_C99_METHOD_PROMISE_ALL_SETTLED),
    emit_method_define!(KILN_C99_METHOD_PROMISE_ANY),
    emit_method_define!(KILN_C99_METHOD_PROMISE_WITH_RESOLVERS),
    emit_method_define!(KILN_C99_METHOD_NODE_TTY_ISATTY),
    emit_method_define!(KILN_C99_METHOD_TYPED_TO_STRING_TAG),
    emit_method_define!(KILN_C99_METHOD_REJECT_STEP),
    r#"
/* The two halves of `finally`, bound to the handler. They run it and then restore the outcome
 * they were passing through, which is the whole difference from `then(f, f)`. */
"#,
    emit_method_define!(KILN_C99_METHOD_FINALLY_PASS),
    emit_method_define!(KILN_C99_METHOD_FINALLY_RAISE),
    r#"/* ...and the two that put the outcome back once the handler's own result has settled. Bound to
 * the outcome, so the pair needs no state object either. */
"#,
    emit_method_define!(KILN_C99_METHOD_FINALLY_RESTORE),
    emit_method_define!(KILN_C99_METHOD_FINALLY_RERAISE),
    r#"
/* Resolving a promise *with* a promise. The attach is a job of its own, which is a turn a program
 * can see -- so it is a step rather than something `settle` does on the spot. */
"#,
    emit_method_define!(KILN_C99_METHOD_ADOPT_STEP),
    r#"
/* `Object.freeze` and `Object.isFrozen`, outside the contiguous `Object` range because it was
 * already closed. */
"#,
    emit_method_define!(KILN_C99_METHOD_OBJECT_FREEZE),
    emit_method_define!(KILN_C99_METHOD_OBJECT_IS_FROZEN),
    r#"
/* What every callable answers to, whatever kind of callable it is:
 * `Object.prototype.hasOwnProperty.call(x, k)` is how a transpiled module asks whether a key is
 * own, and `Array.prototype.slice.call(arguments)` is how it makes an array. */
"#,
    emit_method_define!(KILN_C99_METHOD_FUNCTION_CALL),
    emit_method_define!(KILN_C99_METHOD_FUNCTION_APPLY),
    emit_method_define!(KILN_C99_METHOD_FUNCTION_BIND),
    emit_method_define!(KILN_C99_METHOD_FUNCTION_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_ARRAY_BUFFER_IS_VIEW),
    r#"
/* The reflective statics, and the one instance method that walks a chain. */
"#,
    emit_method_define!(KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF),
    emit_method_define!(KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF),
    emit_method_define!(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_NAMES),
    emit_method_define!(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_SYMBOLS),
    emit_method_define!(KILN_C99_METHOD_OBJECT_FROM_ENTRIES),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FROM),
    emit_method_define!(KILN_C99_METHOD_IS_PROTOTYPE_OF),
    emit_method_define!(KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE),
    r#"
/* `String.raw`, the one built-in written to be used as a template tag. Numbered with the
 * receiver-less statics because that is what it is: the template object arrives as an argument,
 * not as `this`. */
"#,
    emit_method_define!(KILN_C99_METHOD_STRING_RAW),
    emit_method_define!(KILN_C99_METHOD_STRING_FROM_CHAR_CODE),
    r#"
/* The descriptor half of the reflective statics: the operations that talk about a property
 * rather than through it. Numbered in the receiver-less range, because every one of them takes
 * its subject as an argument. */
"#,
    emit_method_define!(KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY),
    emit_method_define!(KILN_C99_METHOD_OBJECT_DEFINE_PROPERTIES),
    emit_method_define!(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTOR),
    emit_method_define!(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTORS),
    r#"
/* The global symbol registry. `Symbol.for` is deliberately separate from `Symbol()` because
 * equal registry keys must return one identity across every module in the realm. */
"#,
    emit_method_define!(KILN_C99_METHOD_SYMBOL_FOR),
    emit_method_define!(KILN_C99_METHOD_SYMBOL_KEY_FOR),
    emit_method_define!(KILN_C99_METHOD_NODE_STAT_IS_FILE),
    emit_method_define!(KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY),
    emit_method_define!(KILN_C99_METHOD_ABORT_CONTROLLER_ABORT),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_ABORT),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_LISTEN),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_UNLISTEN),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_THROW),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_TIMEOUT),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_ANY),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_SETTLE),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_FOLLOW),
    emit_method_define!(KILN_C99_METHOD_TEXT_ENCODER_ENCODE),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE),
    emit_method_define!(KILN_C99_METHOD_NODE_QUERYSTRING_PARSE),
    r#"
/* `Object.is`: `===` except that it separates the two zeroes and finds NaN. */
"#,
    emit_method_define!(KILN_C99_METHOD_OBJECT_IS),
    r#"
/* `Error.captureStackTrace`. Present so `typeof` reports "function", which is what libraries
 * feature-detect on; calling it evaluates to undefined rather than inventing a stack this engine
 * does not record. The interpreter makes exactly that trade, and the two have to make it
 * together. */
"#,
    emit_method_define!(KILN_C99_METHOD_ERROR_CAPTURE_STACK_TRACE),
    r#"
/* `Reflect`: the operations a proxy handler forwards with, as ordinary functions.
 *
 * They are not aliases for the `Object` statics that look like them. `Reflect.get` takes a
 * *receiver*, which is what a forwarding handler has to pass on for an inherited accessor to see
 * the proxy; `Reflect.set` and `Reflect.deleteProperty` answer with a boolean where the operator
 * forms answer with the value; and none of them coerces a primitive target -- `Reflect.get(1, k)`
 * is a TypeError where `(1)[k]` is a read off the string prototype. */
"#,
    emit_method_define!(KILN_C99_METHOD_REFLECT_OWN_KEYS),
    emit_method_define!(KILN_C99_METHOD_REFLECT_GET),
    emit_method_define!(KILN_C99_METHOD_REFLECT_SET),
    emit_method_define!(KILN_C99_METHOD_REFLECT_HAS),
    emit_method_define!(KILN_C99_METHOD_REFLECT_DELETE_PROPERTY),
    emit_method_define!(KILN_C99_METHOD_REFLECT_APPLY),
    emit_method_define!(KILN_C99_METHOD_REFLECT_CONSTRUCT),
    r#"
/* `Proxy.revocable(target, handler)` and the function it hands back. The second is reached only
 * through the first, as a bound method whose receiver is the proxy it retires -- which is what
 * gives the returned function the one capability it needs and nothing else. */
"#,
    emit_method_define!(KILN_C99_METHOD_PROXY_REVOCABLE),
    emit_method_define!(KILN_C99_METHOD_PROXY_REVOKE),
    r#"
/* `Math`, part two -- numbered here rather than beside the first range because that range ends
 * where `Number`'s begins. Still below the array boundary, because these take no receiver. */
"#,
    emit_method_define!(KILN_C99_METHOD_MATH_RANDOM),
    emit_method_define!(KILN_C99_METHOD_MATH_SINH),
    emit_method_define!(KILN_C99_METHOD_MATH_COSH),
    emit_method_define!(KILN_C99_METHOD_MATH_TANH),
    emit_method_define!(KILN_C99_METHOD_MATH_ASINH),
    emit_method_define!(KILN_C99_METHOD_MATH_ACOSH),
    emit_method_define!(KILN_C99_METHOD_MATH_ATANH),
    emit_method_define!(KILN_C99_METHOD_MATH_LOG1P),
    emit_method_define!(KILN_C99_METHOD_MATH_EXPM1),
    emit_method_define!(KILN_C99_METHOD_MATH_FROUND),
    emit_method_define!(KILN_C99_METHOD_MATH_CLZ32),
    emit_method_define!(KILN_C99_METHOD_MATH_IMUL),
    r#"
/* What a regular expression answers to. Below the array boundary because they take a receiver
 * exactly as the number methods do. */
"#,
    emit_method_define!(KILN_C99_METHOD_REGEXP_TEST),
    emit_method_define!(KILN_C99_METHOD_REGEXP_EXEC),
    emit_method_define!(KILN_C99_METHOD_REGEXP_TO_STRING),
    r#"/* A collection's own `next`, distinct from the array iterator's because it walks the collection
 * itself rather than a copy of it -- which is the only way a delete during a loop can be seen.
 * Numbered here rather than beside the collection block because the guard that routes a code to
 * a receiver kind is a range, and this has to land inside the one that answers a method reached
 * through an object. */
"#,
    emit_method_define!(KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT),
    r#"/* `toString` on a string or a boolean, which is the value itself rather than `[object String]`.
 * Library code reaches for it constantly on values it has already coerced. */
"#,
    emit_method_define!(KILN_C99_METHOD_PRIMITIVE_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_TYPED_SUBARRAY),
    emit_method_define!(KILN_C99_METHOD_TYPED_SET),
    emit_method_define!(KILN_C99_METHOD_TYPED_FILL),
    r#"/* The two statics a concrete view constructor carries. Kept below the string block that
 * begins at 240, which `kiln_c99_invoke_method` routes by range. */
"#,
    emit_method_define!(KILN_C99_METHOD_TYPED_OF),
    emit_method_define!(KILN_C99_METHOD_TYPED_FROM),
    r#"/* `Array.prototype.map/filter` keep their generic semantics when borrowed onto a typed array.
 * They need distinct codes from the intrinsic typed-array methods, which return a typed view. */
"#,
    emit_method_define!(KILN_C99_METHOD_ARRAY_GENERIC_MAP),
    emit_method_define!(KILN_C99_METHOD_ARRAY_GENERIC_FILTER),
    r#"/* Runtime-module methods. They use the same canonical call path as language intrinsics while
 * keeping their state on ordinary backend objects. Appended outside every range-dispatch block
 * so adding a runtime facade never changes the meaning of an existing intrinsic code. */
"#,
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_ON),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_ONCE),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_EMIT),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_LISTENERS),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_REMOVE_LISTENER),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_REMOVE_ALL_LISTENERS),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_SET_MAX_LISTENERS),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_EMITTER),
    emit_method_define!(KILN_C99_METHOD_NODE_NET_CREATE_SERVER),
    emit_method_define!(KILN_C99_METHOD_NODE_SERVER_LISTEN),
    emit_method_define!(KILN_C99_METHOD_NODE_SERVER_CLOSE),
    emit_method_define!(KILN_C99_METHOD_NODE_SERVER_ADDRESS),
    emit_method_define!(KILN_C99_METHOD_NODE_SOCKET_WRITE),
    emit_method_define!(KILN_C99_METHOD_NODE_SOCKET_END),
    emit_method_define!(KILN_C99_METHOD_NODE_SOCKET_DESTROY),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_SET_HEADER),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_WRITE_HEAD),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_WRITE),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_END),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP_GET),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP_REQUEST),
    emit_method_define!(KILN_C99_METHOD_NODE_CLIENT_REQUEST_WRITE),
    emit_method_define!(KILN_C99_METHOD_NODE_CLIENT_REQUEST_END),
    emit_method_define!(KILN_C99_METHOD_NODE_REQUEST_HEADERS_GET),
    emit_method_define!(KILN_C99_METHOD_NODE_REQUEST_HEADERS_SET),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_SET_ENCODING),
    emit_method_define!(KILN_C99_METHOD_NODE_NET_CONNECT),
    emit_method_define!(KILN_C99_METHOD_URL_SEARCH_PARAMS_GET),
    emit_method_define!(KILN_C99_METHOD_BUFFER_FROM),
    emit_method_define!(KILN_C99_METHOD_BUFFER_ALLOC),
    emit_method_define!(KILN_C99_METHOD_BUFFER_IS_BUFFER),
    emit_method_define!(KILN_C99_METHOD_BUFFER_BYTE_LENGTH),
    emit_method_define!(KILN_C99_METHOD_BUFFER_CONCAT),
    emit_method_define!(KILN_C99_METHOD_BUFFER_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_NODE_UTIL_DEBUGLOG),
    emit_method_define!(KILN_C99_METHOD_NODE_UTIL_NOOP),
    emit_method_define!(KILN_C99_METHOD_NODE_UTIL_DEPRECATE),
    emit_method_define!(KILN_C99_METHOD_NODE_UTIL_INSPECT),
    emit_method_define!(KILN_C99_METHOD_NODE_UTIL_INHERITS),
    emit_method_define!(KILN_C99_METHOD_NODE_UTIL_FORMAT),
    emit_method_define!(KILN_C99_METHOD_NODE_DIAGNOSTICS_CHANNEL),
    emit_method_define!(KILN_C99_METHOD_NODE_DIAGNOSTICS_TRACING_CHANNEL),
    emit_method_define!(KILN_C99_METHOD_NODE_DIAGNOSTICS_PUBLISH),
    emit_method_define!(KILN_C99_METHOD_NODE_DIAGNOSTICS_RUN_STORES),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTPS_CREATE_SERVER),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP2_CREATE_SERVER),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP2_CREATE_SECURE_SERVER),
    emit_method_define!(KILN_C99_METHOD_NODE_DNS_LOOKUP),
    emit_method_define!(KILN_C99_METHOD_NODE_OS_HOSTNAME),
    emit_method_define!(KILN_C99_METHOD_NODE_OS_NETWORK_INTERFACES),
    emit_method_define!(KILN_C99_METHOD_NODE_STREAM),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE),
    emit_method_define!(KILN_C99_METHOD_NODE_WRITABLE),
    emit_method_define!(KILN_C99_METHOD_NODE_STREAM_ADD_ABORT_SIGNAL),
    emit_method_define!(KILN_C99_METHOD_NODE_STREAM_FINISHED),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_PUSH),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_READ),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_DESTROY),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_PAUSE),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_RESUME),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_PIPE),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_UNPIPE),
    emit_method_define!(KILN_C99_METHOD_NODE_WRITABLE_WRITE),
    emit_method_define!(KILN_C99_METHOD_NODE_WRITABLE_END),
    emit_method_define!(KILN_C99_METHOD_NODE_WRITABLE_DESTROY),
    emit_method_define!(KILN_C99_METHOD_NODE_PERFORMANCE_NOW),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_EXISTS_SYNC),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_READ_FILE),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_STAT),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_WRITE_FILE),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_IS_ABSOLUTE),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_JOIN),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_RESOLVE),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_DIRNAME),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_BASENAME),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_EXTNAME),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_RELATIVE),
    emit_method_define!(KILN_C99_METHOD_NODE_ASSERT_OK),
    emit_method_define!(KILN_C99_METHOD_NODE_WORKER_CONSTRUCTOR),
    emit_method_define!(KILN_C99_METHOD_NODE_MODULE_CREATE_REQUIRE),
    emit_method_define!(KILN_C99_METHOD_NODE_URL_FILE_URL_TO_PATH),
    emit_method_define!(KILN_C99_METHOD_NODE_URL_PARSE),
    emit_method_define!(KILN_C99_METHOD_NODE_URL_PATH_TO_FILE_URL),
    emit_method_define!(KILN_C99_METHOD_NODE_ASYNC_RESOURCE),
    emit_method_define!(KILN_C99_METHOD_NODE_ASYNC_RESOURCE_BIND),
    emit_method_define!(KILN_C99_METHOD_NODE_ASYNC_RESOURCE_DESTROY),
    emit_method_define!(KILN_C99_METHOD_NODE_CRYPTO_RANDOM_UUID),
    emit_method_define!(KILN_C99_METHOD_NODE_CRYPTO_CREATE_HASH),
    emit_method_define!(KILN_C99_METHOD_PROCESS_NEXT_TICK),
    emit_method_define!(KILN_C99_METHOD_PROCESS_HRTIME),
    emit_method_define!(KILN_C99_METHOD_PROCESS_HRTIME_BIGINT),
    emit_method_define!(KILN_C99_METHOD_PROCESS_CWD),
    emit_method_define!(KILN_C99_METHOD_ATOMICS_UNAVAILABLE),
    emit_method_define!(KILN_C99_METHOD_CONSOLE_LOG),
    emit_method_define!(KILN_C99_METHOD_CONSOLE_UNAVAILABLE),
    emit_method_define!(KILN_C99_METHOD_SET_IMMEDIATE),
    emit_method_define!(KILN_C99_METHOD_CLEAR_IMMEDIATE),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP_SERVER),
    emit_method_define!(KILN_C99_METHOD_NODE_SERVER_SET_TIMEOUT),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_GET_HEADER),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_GET_HEADERS),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_HAS_HEADER),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_REMOVE_HEADER),
    r#"/* `Temporal` methods. Numbered above the node block and routed by an explicit range at the top of
 * `kiln_c99_invoke_method`, because the string range below this one is open-ended. */
/* `from` and `compare` are one function per type in the language -- `Temporal.PlainDate.from`
 * accepts a different bag from `Temporal.Duration.from` -- and the call path carries only the
 * method code, not the value the method was reached through. So the kind rides in the code: the
 * eight are contiguous and the base is subtracted to recover it. */
"#,
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_FROM),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_FROM_LAST),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_COMPARE),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_COMPARE_LAST),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_TO_JSON),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_VALUE_OF),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_NOW_TIME_ZONE_ID),
    r#"/* `Temporal.Now`'s five readers, contiguous so the kind rides in the code the way it does for
 * `from` and `compare`. The clock they read is the realm's, which ADR 0015 makes virtual and UTC.
 * The order is the eight-type order, restricted to the five the namespace offers. */
"#,
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_NOW_READ),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_NOW_READ_LAST),
    r#"/* The arithmetic and conversion surface. One code per name, contiguous, because the call path
 * carries only the code -- and the provider is keyed by the name, so the two are the same table
 * read from either end (`KILN_C99_TEMPORAL_OPS` below). */
"#,
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_OP),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_OP_LAST),
    r#"/* `Temporal.Instant`'s two epoch constructors. Statics rather than `from`, and one code each
 * rather than a kind-bearing range, because only `Instant` has them: an epoch count names a point
 * on the timeline, and no other one of the eight is a point on a timeline by itself. */
"#,
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_MILLISECONDS),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_LAST),
    r#"
/* The writers that mirror the readers at 74..82. Contiguous, because a range is how a receiver's
 * methods are found by name, and numbered here rather than beside the readers because that block
 * is full. */
"#,
    emit_method_define!(KILN_C99_METHOD_SET_TIME),
    emit_method_define!(KILN_C99_METHOD_SET_FULL_YEAR),
    emit_method_define!(KILN_C99_METHOD_SET_MONTH),
    emit_method_define!(KILN_C99_METHOD_SET_DATE),
    emit_method_define!(KILN_C99_METHOD_SET_HOURS),
    emit_method_define!(KILN_C99_METHOD_SET_MINUTES),
    emit_method_define!(KILN_C99_METHOD_SET_SECONDS),
    emit_method_define!(KILN_C99_METHOD_SET_MILLISECONDS),
    r#"
/* Dispatch membership is generated from the Rust method registry.  Keep these predicates exact:
 * numeric adjacency is an implementation detail, not a category contract. */
#define KILN_C99_GROUP_DIRECT UINT32_C(1)
#define KILN_C99_GROUP_ARRAY_LOOKUP UINT32_C(2)
#define KILN_C99_GROUP_STRING_DISPATCH UINT32_C(4)
#define KILN_C99_GROUP_GENERIC UINT32_C(8)
#define KILN_C99_GROUP_TEMPORAL UINT32_C(16)
#define KILN_C99_GROUP_TEMPORAL_FROM UINT32_C(32)
#define KILN_C99_GROUP_TEMPORAL_COMPARE UINT32_C(64)
#define KILN_C99_GROUP_TEMPORAL_NOW_READ UINT32_C(128)
#define KILN_C99_GROUP_TEMPORAL_OP UINT32_C(256)
#define KILN_C99_GROUP_DATE_SETTER UINT32_C(512)
#define KILN_C99_GROUP_NODE_EVENT UINT32_C(1024)
#define KILN_C99_GROUP_NODE_NET UINT32_C(2048)
#define KILN_C99_GROUP_NODE_SERVER UINT32_C(4096)
#define KILN_C99_GROUP_NODE_HTTP UINT32_C(8192)
#define KILN_C99_GROUP_NODE_RESPONSE UINT32_C(16384)
#define KILN_C99_GROUP_BUFFER UINT32_C(32768)
#define KILN_C99_GROUP_NODE_UTIL UINT32_C(65536)
#define KILN_C99_GROUP_NODE_DIAGNOSTICS UINT32_C(131072)
#define KILN_C99_GROUP_NODE_STREAM UINT32_C(262144)
#define KILN_C99_GROUP_NODE_PATH UINT32_C(524288)
#define KILN_C99_GROUP_MATH_NUMBER UINT32_C(1048576)

static uint32_t kiln_c99_method_groups(uint64_t code) {
    /* Four `Temporal` families are *ranges* rather than lists. `from`, `compare` and `Now.x` are
     * one code per kind and the twenty-three operations are one code each, all contiguous, so the
     * code carries which kind or which operation was asked for -- `kind = code - FROM` is how the
     * dispatch recovers it, rather than a translation table that could drift from the provider's
     * own order.
     *
     * The switch below is generated from the id list, which names only the two *endpoints* of
     * each range. Every code between them therefore answered "no groups at all", which routed it
     * past the `Temporal` dispatch entirely and out the refusal at the bottom. That is why
     * `Temporal.PlainDate.from` worked and the other seven did not: 285 is named and 286 is not.
     * The same hole took seven of the eight `compare`s, seven of the eight `Temporal.Now` reads,
     * and twenty-one of the twenty-three operations.
     *
     * A range is checked as a range. The endpoints stay in the id list because the Rust
     * assertions above measure the ranges from them. */
    if (code >= KILN_C99_METHOD_TEMPORAL_FROM && code <= KILN_C99_METHOD_TEMPORAL_FROM_LAST) {
        return KILN_C99_GROUP_TEMPORAL | KILN_C99_GROUP_TEMPORAL_FROM;
    }
    if (code >= KILN_C99_METHOD_TEMPORAL_COMPARE && code <= KILN_C99_METHOD_TEMPORAL_COMPARE_LAST) {
        return KILN_C99_GROUP_TEMPORAL | KILN_C99_GROUP_TEMPORAL_COMPARE;
    }
    if (code >= KILN_C99_METHOD_TEMPORAL_NOW_READ &&
        code <= KILN_C99_METHOD_TEMPORAL_NOW_READ_LAST) {
        return KILN_C99_GROUP_TEMPORAL | KILN_C99_GROUP_TEMPORAL_NOW_READ;
    }
    if (code >= KILN_C99_METHOD_TEMPORAL_OP && code <= KILN_C99_METHOD_TEMPORAL_OP_LAST) {
        return KILN_C99_GROUP_TEMPORAL | KILN_C99_GROUP_TEMPORAL_OP;
    }
    switch (code) {
"#,
    emit_method_group_cases!(),
    r#"
    default: return UINT32_C(0);
    }
}

static int kiln_c99_method_in_group(uint64_t code, uint32_t group) {
    return (kiln_c99_method_groups(code) & group) != 0;
}
"#,
    include_str!("runtime/prelude_types.h"),
    kiln_native_number_abi::provider_abi_registry!(provider_http_close_c_declaration),
    include_str!("runtime/prelude_runtime.c"),
    emit_method_code_list!(),
    r#"};

static uint64_t kiln_c99_lookup_method_group(const char *key, size_t key_len, uint32_t group) {
    size_t index;
    for (index = 0; index < sizeof kiln_c99_method_codes / sizeof *kiln_c99_method_codes; ++index) {
        uint64_t code = kiln_c99_method_codes[index];
        if (!kiln_c99_method_in_group(code, group)) {
            continue;
        }
        const char *name = kiln_c99_method_name(code);
        size_t len;
        if (name == NULL) {
            continue;
        }
        len = strlen(name);
        if (len == key_len && memcmp(name, key, len) == 0) {
            return code;
        }
    }
    return 0;
}

/* A few prototype tables intentionally share a code span with another table. Their existing
 * explicit endpoints remain a lookup detail; the receiver-dispatch paths use generated groups. */
static uint64_t kiln_c99_lookup_method(const char *key, size_t key_len, uint64_t first,
                                       uint64_t last) {
    uint64_t code;
    for (code = first; code <= last; ++code) {
        const char *name = kiln_c99_method_name(code);
        size_t len;
        if (name == NULL) {
            continue;
        }
        len = strlen(name);
        if (len == key_len && memcmp(name, key, len) == 0) {
            return code;
        }
    }
    return 0;
}

static uint64_t kiln_c99_array_method(const char *key, size_t key_len) {
    return kiln_c99_lookup_method_group(key, key_len, KILN_C99_GROUP_ARRAY_LOOKUP);
}

/* Whether a name is a member of `Array.prototype` *in the language*, and the same question for
 * `Date.prototype`. Both are the string list's question, one type over, and both were missing.
 *
 * `a["nope"]` and `date["nope"]` are not gaps in this backend -- they are what the language
 * answers, which is undefined -- and refusing them ended the process where it should have
 * answered. A bundled `vue-reactivity` reads arbitrary names off an array because that is what a
 * reactive proxy does, and `dayjs` does the same to a date; both stopped here.
 *
 * Keeping the list is what preserves the other half. A name that really is a member but is not
 * implemented yet still refuses by name, so `a.toSorted(f)` says so rather than becoming
 * "undefined is not a function" three frames from the cause -- and, worse, rather than quietly
 * failing a `typeof a.toSorted === "function"` feature test. */
static int kiln_c99_is_array_member(const char *key, size_t key_len) {
    static const char *const members[] = {
        "at", "concat", "constructor", "copyWithin", "entries", "every", "fill", "filter", "find",
        "findIndex", "findLast", "findLastIndex", "flat", "flatMap", "forEach", "includes",
        "indexOf", "join", "keys", "lastIndexOf", "length", "map", "pop", "push", "reduce",
        "reduceRight", "reverse", "shift", "slice", "some", "sort", "splice", "toLocaleString",
        "toReversed", "toSorted", "toSpliced", "toString", "unshift", "values", "with"};
    size_t index;
    for (index = 0; index < sizeof members / sizeof *members; ++index) {
        if (kiln_c99_same_name(key, key_len, members[index])) {
            return 1;
        }
    }
    /* `@@iterator` and `@@unscopables` are members too, and reserved keys reach here spelled with
     * the `@@` prefix. */
    return key_len > 2 && key[0] == '@' && key[1] == '@';
}

static int kiln_c99_is_date_member(const char *key, size_t key_len) {
    static const char *const members[] = {
        "constructor", "getDate", "getDay", "getFullYear", "getHours", "getMilliseconds",
        "getMinutes", "getMonth", "getSeconds", "getTime", "getTimezoneOffset", "getUTCDate",
        "getUTCDay", "getUTCFullYear", "getUTCHours", "getUTCMilliseconds", "getUTCMinutes",
        "getUTCMonth", "getUTCSeconds", "getYear", "setDate", "setFullYear", "setHours",
        "setMilliseconds", "setMinutes", "setMonth", "setSeconds", "setTime", "setUTCDate",
        "setUTCFullYear", "setUTCHours", "setUTCMilliseconds", "setUTCMinutes", "setUTCMonth",
        "setUTCSeconds", "setYear", "toDateString", "toISOString", "toJSON", "toLocaleDateString",
        "toLocaleString", "toLocaleTimeString", "toString", "toTimeString", "toUTCString",
        "valueOf"};
    size_t index;
    for (index = 0; index < sizeof members / sizeof *members; ++index) {
        if (kiln_c99_same_name(key, key_len, members[index])) {
            return 1;
        }
    }
    return key_len > 2 && key[0] == '@' && key[1] == '@';
}

/* Whether a name is a member of `String.prototype` *in the language*, which is a different
 * question from whether this backend implements it.
 *
 * A string's properties are its indices, `length`, and these; anything else a program names is
 * absent, and absent is `undefined`. Refusing every unrecognised name was the bug -- `"abc"[-1]`,
 * `"abc"[1.5]` and `"abc"["nope"]` are not gaps in this backend, they are what the language
 * answers -- and refusing ended the process where it should have answered. Keeping the list is
 * what preserves the other half: a name that really is a member but is not implemented yet still
 * refuses by name, which is the diagnostic the refusal exists to give. */
static int kiln_c99_is_string_member(const char *key, size_t key_len) {
    static const char *const members[] = {
        "anchor",       "at",         "big",           "blink",         "bold",
        "charAt",       "charCodeAt", "codePointAt",   "concat",        "constructor",
        "endsWith",     "fixed",      "fontcolor",     "fontsize",      "includes",
        "indexOf",      "isWellFormed", "italics",     "lastIndexOf",   "link",
        "localeCompare","match",      "matchAll",      "normalize",     "padEnd",
        "padStart",     "repeat",     "replace",       "replaceAll",    "search",
        "slice",        "small",      "split",         "startsWith",    "strike",
        "sub",          "substr",     "substring",     "sup",           "toLocaleLowerCase",
        "toLocaleUpperCase", "toLowerCase", "toString", "toUpperCase",  "toWellFormed",
        "trim",         "trimEnd",    "trimLeft",      "trimRight",     "trimStart",
        "valueOf"};
    size_t index;
    for (index = 0; index < sizeof members / sizeof *members; ++index) {
        if (kiln_c99_same_name(key, key_len, members[index])) {
            return 1;
        }
    }
    return 0;
}

/* The fixed-length subset inherited from `%TypedArray%.prototype`.
 *
 * `%TypedArray%.prototype` is deliberately *not* `Array.prototype`: it shares the readers and the
 * two in-place rearrangements, and it has none of the methods that change a length, because a
 * view's length is its buffer's and cannot change. That is why the table is written out here
 * rather than delegating to `kiln_c99_array_method`, which would hand out `push` -- a name node
 * answers `undefined` for and a program feature-detects on. */
static uint64_t kiln_c99_typed_prototype_method(const char *key, size_t key_len) {
    if (kiln_c99_same_name(key, key_len, "join")) return KILN_C99_METHOD_JOIN;
    if (kiln_c99_same_name(key, key_len, "values")) return KILN_C99_METHOD_ARRAY_VALUES;
    if (kiln_c99_same_name(key, key_len, "entries")) return KILN_C99_METHOD_ARRAY_ENTRIES;
    if (kiln_c99_same_name(key, key_len, "keys")) return KILN_C99_METHOD_ARRAY_KEYS;
    if (kiln_c99_same_name(key, key_len, "subarray")) return KILN_C99_METHOD_TYPED_SUBARRAY;
    if (kiln_c99_same_name(key, key_len, "slice")) return KILN_C99_METHOD_SLICE;
    if (kiln_c99_same_name(key, key_len, "set")) return KILN_C99_METHOD_TYPED_SET;
    if (kiln_c99_same_name(key, key_len, "fill")) return KILN_C99_METHOD_TYPED_FILL;
    /* `toString` is `Array.prototype.toString`, which joins -- not `Object.prototype.toString`,
     * which reports the kind. Falling through to the generic one is how `String(view)` came to
     * be "[object Uint8Array]" where both node and the interpreter say "1,2,3". */
    if (kiln_c99_same_name(key, key_len, "toString")) return KILN_C99_METHOD_ARRAY_TO_STRING;
    if (kiln_c99_same_name(key, key_len, "at")) return KILN_C99_METHOD_ARRAY_AT;
    if (kiln_c99_same_name(key, key_len, "indexOf")) return KILN_C99_METHOD_INDEX_OF;
    if (kiln_c99_same_name(key, key_len, "lastIndexOf")) return KILN_C99_METHOD_ARRAY_LAST_INDEX_OF;
    if (kiln_c99_same_name(key, key_len, "includes")) return KILN_C99_METHOD_INCLUDES;
    if (kiln_c99_same_name(key, key_len, "find")) return KILN_C99_METHOD_ARRAY_FIND;
    if (kiln_c99_same_name(key, key_len, "findIndex")) return KILN_C99_METHOD_ARRAY_FIND_INDEX;
    if (kiln_c99_same_name(key, key_len, "findLast")) return KILN_C99_METHOD_ARRAY_FIND_LAST;
    if (kiln_c99_same_name(key, key_len, "findLastIndex"))
        return KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX;
    if (kiln_c99_same_name(key, key_len, "forEach")) return KILN_C99_METHOD_FOR_EACH;
    if (kiln_c99_same_name(key, key_len, "some")) return KILN_C99_METHOD_ARRAY_SOME;
    if (kiln_c99_same_name(key, key_len, "every")) return KILN_C99_METHOD_ARRAY_EVERY;
    if (kiln_c99_same_name(key, key_len, "reduce")) return KILN_C99_METHOD_REDUCE;
    if (kiln_c99_same_name(key, key_len, "reduceRight")) return KILN_C99_METHOD_ARRAY_REDUCE_RIGHT;
    if (kiln_c99_same_name(key, key_len, "map")) return KILN_C99_METHOD_MAP;
    if (kiln_c99_same_name(key, key_len, "filter")) return KILN_C99_METHOD_FILTER;
    if (kiln_c99_same_name(key, key_len, "sort")) return KILN_C99_METHOD_ARRAY_SORT;
    if (kiln_c99_same_name(key, key_len, "reverse")) return KILN_C99_METHOD_ARRAY_REVERSE;
    return 0;
}

/* Whether a name is a member of `%TypedArray%.prototype` *in the language*, which is a different
 * question from whether this backend implements it -- the same distinction
 * `kiln_c99_is_string_member` draws, and for the same reason.
 *
 * A view's properties are its live indices, its buffer metadata, and the methods above. Anything
 * else a program names is absent, and absent is `undefined`. These are the names that are not
 * absent and are not implemented either: answering `undefined` for one would be a silent wrong
 * answer, so a read of one refuses instead and lands in the backlog by name. */
static int kiln_c99_is_typed_array_member(const char *key, size_t key_len) {
    static const char *const members[] = {"copyWithin", "toLocaleString", "toReversed",
                                          "toSorted",   "with"};
    size_t index;
    for (index = 0; index < sizeof members / sizeof *members; ++index) {
        if (kiln_c99_same_name(key, key_len, members[index])) {
            return 1;
        }
    }
    return 0;
}

static uint64_t kiln_c99_string_method(const char *key, size_t key_len) {
    return kiln_c99_lookup_method_group(key, key_len, KILN_C99_GROUP_STRING_DISPATCH);
}

/* The static surface of every built-in this backend answers for: one row per (built-in, member).
 *
 * ONE TABLE, TWO EMITTERS (ADR 0043). Every row below is generated from `BUILTIN_SURFACE` in this
 * backend's Rust, which is also what the drift test reads. Before it the same surface was written
 * down twice -- once in the interpreter and once as `kiln_c99_same_name` chains here -- and
 * nothing compared them, so a member the interpreter grew was discovered by whichever npm bundle
 * happened to read it. `Number` had fifteen statics on one side and five here; `Number.MAX_VALUE`
 * is the first property a bundled `zod` reads.
 *
 * A row carries what the member *is*, not merely that it exists, so the read path, the call path
 * and `Object.getOwnPropertyNames` all answer out of it. That is what makes a member which is
 * implemented but unreachable inexpressible: reachability is the row. A `@@` name is symbol-keyed
 * and is reached through a Symbol rather than as a string key. */
#define KILN_C99_MEMBER_METHOD 1
#define KILN_C99_MEMBER_NUMBER 2
#define KILN_C99_MEMBER_TEXT 3
#define KILN_C99_MEMBER_SYMBOL 4
#define KILN_C99_MEMBER_BUILTIN 5

typedef struct {
    uint16_t builtin;
    uint16_t kind;
    /* A method code, or the built-in a `KILN_C99_MEMBER_BUILTIN` row hands back. */
    uint16_t code;
    double number;
    const char *name;
    /* The `@@` key of a well-known symbol, or the text a tag answers with. */
    const char *text;
} kiln_builtin_member;

static const kiln_builtin_member kiln_c99_builtin_members[] = {
"#,
    emit_builtin_surface_rows!(),
    r#"};

#define KILN_C99_SURFACE_MEMBER_COUNT \
    (sizeof kiln_c99_builtin_members / sizeof *kiln_c99_builtin_members)

/* The members this backend knows the language has and it does not answer, by the global that owns
 * them. Generated from the same declaration as the Rust list, so the two cannot disagree.
 *
 * The read path needs it to tell apart two things that are the same absence inside the surface
 * table: a name the language never put on this built-in, which is `undefined`, and a member that
 * belongs there and is not built yet, which must refuse by name. Answering `undefined` for the
 * second would break `typeof Object.groupBy === "function"` -- the program takes the other branch
 * and nothing reports anything, which is the failure this backend exists to avoid. */
typedef struct {
    const char *owner;
    const char *member;
} kiln_builtin_gap;

static const kiln_builtin_gap kiln_c99_builtin_backlog[] = {
"#,
    emit_builtin_backlog_rows!(),
    include_str!("runtime/prelude_core.c"),
);

/// Mutable static state embedded in the generated-C prelude, classified by its required owner.
///
/// This is the migration manifest for ADR 0036. Adding a new mutable `kiln_c99_*` static without
/// naming its owner fails the architecture test below. Realm entries move into the shared value
/// kernel; frame entries remain save/restore state local to one synchronous generated-body call.
#[cfg(test)]
const C99_MUTABLE_STATE_OWNERS: &[(&str, C99StateOwner)] = &[
    ("kiln_c99_diagnostic_site", C99StateOwner::Realm),
    ("kiln_c99_diagnostic_operation", C99StateOwner::Realm),
    ("kiln_c99_diagnostic_instruction", C99StateOwner::Realm),
    ("kiln_c99_diagnostic_depth", C99StateOwner::Realm),
    ("kiln_c99_diagnostic_method", C99StateOwner::Realm),
    ("kiln_c99_diagnostic_receiver_tag", C99StateOwner::Realm),
    ("kiln_c99_diagnostic_has_method", C99StateOwner::Realm),
    ("kiln_c99_diagnostic_detail", C99StateOwner::Realm),
    ("kiln_c99_diagnostic_function", C99StateOwner::Realm),
    ("kiln_c99_diagnostic_function_len", C99StateOwner::Realm),
    ("kiln_c99_diagnostic_active_depth", C99StateOwner::Frame),
    ("kiln_c99_new_target", C99StateOwner::Frame),
    ("kiln_c99_inheriting_new_target", C99StateOwner::Frame),
    ("kiln_c99_adopted_this", C99StateOwner::Frame),
    ("kiln_c99_frame_args", C99StateOwner::Frame),
    ("kiln_c99_frame_argc", C99StateOwner::Frame),
    ("kiln_c99_captured_arguments", C99StateOwner::Frame),
    ("kiln_c99_frame_arguments_object", C99StateOwner::Frame),
    ("kiln_c99_frame_is_arrow", C99StateOwner::Frame),
    ("kiln_c99_frame_mapped_arguments", C99StateOwner::Frame),
    ("kiln_c99_frame_callee", C99StateOwner::Frame),
    ("kiln_c99_symbols", C99StateOwner::Realm),
    ("kiln_c99_symbol_count", C99StateOwner::Realm),
    ("kiln_c99_symbol_capacity", C99StateOwner::Realm),
    ("kiln_c99_fixture_method_stores", C99StateOwner::TestFixture),
    ("kiln_c99_gc_fallback", C99StateOwner::TestFixture),
    ("kiln_c99_keys_check_mode", C99StateOwner::TestFixture),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(test)]
enum C99StateOwner {
    Realm,
    Frame,
    TestFixture,
}

/// The stack each emitted C function needs: the module body, then one entry per compiled
/// function body in [`Program::functions`], in the same order.
struct StackPlan {
    body: usize,
    functions: Vec<usize>,
}

fn validate(program: &Program) -> Result<usize, EmitError> {
    Ok(validate_program(program)?.body)
}

fn validate_program(program: &Program) -> Result<StackPlan, EmitError> {
    if program.version != BYTECODE_VERSION {
        return Err(EmitError::Unsupported {
            feature: format!(
                "bytecode version {} (backend supports {})",
                program.version, BYTECODE_VERSION
            ),
        });
    }
    if program.code.is_empty() {
        return Err(EmitError::InvalidProgram {
            message: "program has no instructions".into(),
        });
    }
    for proto in &program.functions {
        // A generator does not run when it is called: it builds an object that runs in pieces,
        // each resumption picking up where the last left off. That is a suspended frame, which
        // an automatic C frame cannot be. An `async` function is compiled as one for the same
        // reason -- `await` suspends exactly as `yield` does.
        // An async *generator* suspends for two different reasons -- a value for its consumer
        // and a promise it is waiting on -- and the driver has to tell them apart to know whether
        // the consumer's step is finished. `frame->awaiting`, written at each suspension, is what
        // says which; without it the two emit identically and nothing downstream could tell.
        if proto.parameter_count > proto.local_count {
            return Err(EmitError::InvalidProgram {
                message: format!(
                    "function takes {} parameter(s) but has only {} local slot(s)",
                    proto.parameter_count, proto.local_count
                ),
            });
        }
        if proto.code.is_empty() {
            return Err(EmitError::InvalidProgram {
                message: "function body has no instructions".into(),
            });
        }
    }
    let body = validate_code(&program.code, program, program.local_count, 0)?;
    let functions = program
        .functions
        .iter()
        .map(|proto| {
            validate_code(
                &proto.code,
                program,
                proto.local_count,
                proto.upvalues.len(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StackPlan { body, functions })
}

/// Validate one body -- the module's or a function's -- and report the stack depth it reaches.
///
/// `local_count` and `upvalue_count` describe the frame this body runs in, which is why this
/// cannot simply read them off the program: every function has its own.
fn validate_code(
    code: &[Instruction],
    program: &Program,
    local_count: u32,
    upvalue_count: usize,
) -> Result<usize, EmitError> {
    for (index, instruction) in code.iter().enumerate() {
        validate_supported_instruction(instruction)?;
        match instruction {
            Instruction::Const(constant) if (*constant as usize) >= program.constants.len() => {
                return Err(invalid(
                    index,
                    format!("constant {constant} is out of bounds"),
                ));
            }
            // The interpreter can make a second realm; this backend cannot. Its global object is
            // built once in C, in one copy, so a compiled program that reached the hook would get
            // a plausible-looking realm whose intrinsics were secretly this one's -- and every
            // cross-realm identity question would then be answered confidently and wrongly.
            // Refused by name instead, which is what the interpreter does when its host has not
            // granted the capability.
            Instruction::RequireGlobalBinding(name)
            | Instruction::LoadGlobalBindingOrUndefined(name)
            // A name this backend has no global for never reaches the two above: the compiler
            // emits the lookup and then `ThrowReference` for the miss, so refusing only the
            // lookup let `new ShadowRealm()` compile and answer `ReferenceError` at run time --
            // which reads to a program as "this engine does not have it" rather than "this
            // backend cannot do it", and is the difference between a refusal and a wrong answer
            // about what the language provides.
            | Instruction::ThrowReference(name)
            | Instruction::StoreGlobalBinding(name)
            | Instruction::StoreGlobalBindingStrict(name)
            | Instruction::DeleteGlobalBinding(name)
            | Instruction::DeclareGlobalBinding { name, .. }
            | Instruction::GetProperty(name)
            | Instruction::CallMethod { name, .. }
                if matches!(
                    program.constants.get(*name as usize),
                    Some(Constant::String(text))
                        if refused_host_hook(text)
                            || text == HOST_CREATE_REALM
                            || text == HOST_EVAL_SCRIPT
                            || text == SHADOW_REALM
                ) =>
            {
                let hook = match program.constants.get(*name as usize) {
                    Some(Constant::String(text)) => text.clone(),
                    _ => HOST_CREATE_REALM.to_owned(),
                };
                return Err(EmitError::Unsupported {
                    feature: format!("host realm facility {hook} at instruction {index}"),
                });
            }
            // A property name is emitted by naming the constant's own C symbol, and only a
            // string constant has one.
            Instruction::GetProperty(name)
            | Instruction::SetProperty(name)
            | Instruction::DeleteProperty(name)
            | Instruction::SetPropertyStrict { name, .. }
            | Instruction::DeletePropertyStrict { name, .. }
            | Instruction::DeleteGlobalBinding(name)
            | Instruction::StoreGlobalBindingStrict(name)
            | Instruction::RequireGlobalBinding(name)
            | Instruction::LoadGlobalBindingOrUndefined(name)
            | Instruction::DeclareGlobalBinding { name, .. }
            | Instruction::CallMethod { name, .. } => match program.constants.get(*name as usize) {
                Some(Constant::String(_)) => {}
                Some(_) => {
                    return Err(invalid(
                        index,
                        format!("property name constant {name} is not a string"),
                    ));
                }
                None => {
                    return Err(invalid(
                        index,
                        format!("property name constant {name} is out of bounds"),
                    ));
                }
            },
            Instruction::LoadLocal(local)
            | Instruction::StoreLocal(local)
            | Instruction::NewCell(local)
            | Instruction::LoadCell(local)
            | Instruction::StoreCell(local)
            | Instruction::MarkUninitialized(local)
                if (*local as u64) >= u64::from(local_count) =>
            {
                return Err(invalid(index, format!("local {local} is out of bounds")));
            }
            Instruction::LoadGlobal(slot) | Instruction::StoreGlobal(slot)
                if u64::from(*slot) >= u64::from(program.local_count) =>
            {
                return Err(invalid(
                    index,
                    format!("module slot {slot} is out of bounds"),
                ));
            }
            Instruction::LoadUpvalue(upvalue) | Instruction::StoreUpvalue(upvalue)
                if (*upvalue as usize) >= upvalue_count =>
            {
                return Err(invalid(
                    index,
                    format!("upvalue {upvalue} is out of bounds"),
                ));
            }
            Instruction::Jump(target)
            | Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target)
            | Instruction::PushHandler(target)
                if (*target as usize) >= code.len() =>
            {
                return Err(invalid(
                    index,
                    format!("jump target {target} is out of bounds"),
                ));
            }
            Instruction::HostCall { op, .. } if !matches!(op, HostOp::WriteStdout) => {
                return Err(EmitError::Unsupported {
                    feature: format!("host operation {op:?} at instruction {index}"),
                });
            }
            // Where a closure's captures are checked: the descriptors name slots in *this*
            // frame, so this is the only place that knows whether they exist.
            Instruction::MakeFunction(proto) => {
                let Some(target) = program.functions.get(*proto as usize) else {
                    return Err(invalid(index, format!("function {proto} is out of bounds")));
                };
                for upvalue in &target.upvalues {
                    let ok = match upvalue {
                        Upvalue::ParentLocal(slot) => u64::from(*slot) < u64::from(local_count),
                        Upvalue::ParentUpvalue(slot) => (*slot as usize) < upvalue_count,
                    };
                    if !ok {
                        return Err(invalid(
                            index,
                            format!(
                                "function {proto} captures {upvalue:?}, which this frame does not have"
                            ),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    // Dataflow validation keeps generated stack indexing defined on every reachable branch and
    // calculates a compact allocation for the emitted function.
    let mut heights: Vec<Option<usize>> = vec![None; code.len()];
    let mut work = vec![(0usize, 0usize)];
    let mut max_height = 0usize;
    while let Some((index, height)) = work.pop() {
        if let Some(previous) = heights[index] {
            if previous != height {
                return Err(invalid(
                    index,
                    format!("inconsistent stack height: reached with {previous} and {height}"),
                ));
            }
            continue;
        }
        heights[index] = Some(height);
        max_height = max_height.max(height);
        let instruction = &code[index];
        let (next_height, terminal) = stack_effect(instruction, height, index)?;
        max_height = max_height.max(next_height);
        if terminal {
            continue;
        }
        match instruction {
            // A handler is entered with the thrown value pushed onto the stack as the enclosing
            // `try` left it, which is why the edge carries one more than the fall-through.
            Instruction::PushHandler(target) => {
                work.push((*target as usize, next_height + 1));
                if index + 1 < code.len() {
                    work.push((index + 1, next_height));
                } else {
                    return Err(invalid(index, "control falls off the end of the program"));
                }
            }
            Instruction::Jump(target) => work.push((*target as usize, next_height)),
            Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target) => {
                work.push((*target as usize, next_height));
                if index + 1 < code.len() {
                    work.push((index + 1, next_height));
                }
            }
            _ if index + 1 < code.len() => work.push((index + 1, next_height)),
            _ => {
                return Err(invalid(index, "control falls off the end of the program"));
            }
        }
    }
    Ok(max_height.max(1))
}

/// Whether the backend can emit this instruction.
///
/// There is no catch-all arm any more: **every instruction in the set is emitted**, and the match
/// is exhaustive so that adding one to `kiln-ir` stops this compiling until somebody decides what
/// the backend does about it. A refusal that has to be written down beats one that happens by
/// default -- and the default was the whole reason the tally in NOTES.md kept finding things.
///
/// A *builtin* can still be refused: `LoadBuiltin` names one value out of a large enum and the
/// arm below lists the ones that exist.
fn validate_supported_instruction(instruction: &Instruction) -> Result<(), EmitError> {
    match instruction {
        // A template's `${x}` lowers to a call to `String`, so this is not an optional extra:
        // without it the backend loses every program containing a template literal.
        Instruction::LoadBuiltin(
            kiln_ir::Builtin::String
            | kiln_ir::Builtin::Math
            | kiln_ir::Builtin::Number
            | kiln_ir::Builtin::Object
            | kiln_ir::Builtin::Array
            | kiln_ir::Builtin::Json
            | kiln_ir::Builtin::Date
            | kiln_ir::Builtin::Map
            | kiln_ir::Builtin::Set
            | kiln_ir::Builtin::WeakMap
            | kiln_ir::Builtin::WeakSet
            | kiln_ir::Builtin::Promise
            | kiln_ir::Builtin::Error
            | kiln_ir::Builtin::TypeError
            | kiln_ir::Builtin::RangeError
            | kiln_ir::Builtin::ReferenceError
            | kiln_ir::Builtin::SyntaxError
            | kiln_ir::Builtin::UriError
            | kiln_ir::Builtin::AggregateError
            | kiln_ir::Builtin::NotANumber
            | kiln_ir::Builtin::Infinity
            | kiln_ir::Builtin::Boolean
            | kiln_ir::Builtin::ParseInt
            | kiln_ir::Builtin::ParseFloat
            | kiln_ir::Builtin::IsNaN
            | kiln_ir::Builtin::IsFinite
            | kiln_ir::Builtin::Escape
            | kiln_ir::Builtin::Unescape
            | kiln_ir::Builtin::EncodeUri
            | kiln_ir::Builtin::EncodeUriComponent
            | kiln_ir::Builtin::DecodeUri
            | kiln_ir::Builtin::DecodeUriComponent
            | kiln_ir::Builtin::Reflect
            | kiln_ir::Builtin::Proxy
            | kiln_ir::Builtin::QueueMicrotask
            | kiln_ir::Builtin::SetTimeout
            | kiln_ir::Builtin::SetInterval
            | kiln_ir::Builtin::ClearTimeout
            | kiln_ir::Builtin::ClearInterval
            | kiln_ir::Builtin::Symbol
            | kiln_ir::Builtin::RegExp
            | kiln_ir::Builtin::GlobalThis
            | kiln_ir::Builtin::BigInt
            | kiln_ir::Builtin::Function
            | kiln_ir::Builtin::ArrayBuffer
            | kiln_ir::Builtin::Int8Array
            | kiln_ir::Builtin::Uint8Array
            | kiln_ir::Builtin::Uint8ClampedArray
            | kiln_ir::Builtin::Int16Array
            | kiln_ir::Builtin::Uint16Array
            | kiln_ir::Builtin::Int32Array
            | kiln_ir::Builtin::Uint32Array
            | kiln_ir::Builtin::Float32Array
            | kiln_ir::Builtin::Float64Array
            | kiln_ir::Builtin::Url
            | kiln_ir::Builtin::Buffer
            | kiln_ir::Builtin::Process
            | kiln_ir::Builtin::Atomics
            | kiln_ir::Builtin::SetImmediate
            | kiln_ir::Builtin::ClearImmediate
            | kiln_ir::Builtin::AbortController
            | kiln_ir::Builtin::AbortSignal
            | kiln_ir::Builtin::TextEncoder
            | kiln_ir::Builtin::Performance,
        ) => Ok(()),
        Instruction::LoadBuiltin(builtin) => Err(EmitError::Unsupported {
            feature: format!("the `{builtin:?}` builtin in the native backend"),
        }),
        // A direct `eval` needs the compiler at run time: it reads the caller's bindings and
        // compiles source that does not exist until the call happens. The native backend ships
        // without a compiler, so this is refused rather than approximated -- an indirect `eval`
        // is a different instruction and is unaffected.
        Instruction::DirectEval(_) => Err(EmitError::Unsupported {
            feature: "a direct `eval` call in the native backend".to_owned(),
        }),
        // `import(specifier)` resolves and loads a module while the program runs. A generated
        // unit is a closed object file: the module graph was settled when it was emitted, and
        // there is nothing at run time to resolve a specifier against or to compile what it
        // names. Refused rather than answered with a rejected promise, which would be a wrong
        // answer wearing the right shape.
        Instruction::DynamicImport => Err(EmitError::Unsupported {
            feature: "a dynamic `import()` in the native backend".to_owned(),
        }),
        // `with (x) { ... }` puts an object in front of the scope chain, so every name in the
        // body is resolved at run time against whatever properties that object turns out to
        // have -- and against `Symbol.unscopables`, which can hide one. The backend resolves
        // names when it emits, into slots; there is no scope left at run time for an object to
        // sit in front of. Both halves are refused together, because half of a `with` is not a
        // smaller feature but a wrong one.
        Instruction::ToObject | Instruction::WithHas(_) => Err(EmitError::Unsupported {
            feature: "a `with` statement in the native backend".to_owned(),
        }),
        Instruction::Const(_)
        | Instruction::LoadLocal(_)
        | Instruction::StoreLocal(_)
        | Instruction::NewCell(_)
        | Instruction::LoadCell(_)
        | Instruction::StoreCell(_)
        | Instruction::MakeFunction(_)
        | Instruction::Call { .. }
        | Instruction::LoadUpvalue(_)
        | Instruction::StoreUpvalue(_)
        | Instruction::MarkUninitialized(_)
        | Instruction::NewObject { .. }
        | Instruction::GetProperty(_)
        | Instruction::SetProperty(_)
        | Instruction::SetPropertyStrict { .. }
        | Instruction::NewArray { .. }
        | Instruction::GetComputed
        | Instruction::SetComputed
        | Instruction::SetComputedStrict { .. }
        | Instruction::CallMethod { .. }
        | Instruction::LoadThis
        | Instruction::PushHandler(_)
        | Instruction::PopHandler
        | Instruction::Throw
        | Instruction::ThrowReference(_)
        | Instruction::StoreGlobalBinding(_)
        | Instruction::StoreGlobalBindingStrict(_)
        | Instruction::RequireGlobalBinding(_)
        | Instruction::LoadGlobalBindingOrUndefined(_)
        | Instruction::DeleteGlobalBinding(_)
        | Instruction::DeclareGlobalBinding { .. }
        | Instruction::GetPrivate(_)
        | Instruction::SetPrivate(_)
        | Instruction::Dup2
        | Instruction::CallComputed { .. }
        | Instruction::NewRegExp { .. }
        | Instruction::New { .. }
        | Instruction::SetPrototype
        | Instruction::LoadPrototype
        | Instruction::LoadSuperProperty(_)
        | Instruction::LoadSuperElement
        | Instruction::LoadGlobal(_)
        | Instruction::StoreGlobal(_)
        | Instruction::DefineAccessor { .. }
        | Instruction::DefineMethod
        | Instruction::DefinePrivate { .. }
        | Instruction::SpreadObject
        | Instruction::SpreadInto
        | Instruction::ConstructSpread
        | Instruction::IterableToArray
        | Instruction::EnumerateKeys
        | Instruction::DeleteProperty(_)
        | Instruction::DeletePropertyStrict { .. }
        | Instruction::DeleteComputed
        | Instruction::DeleteComputedStrict { .. }
        | Instruction::Apply
        | Instruction::RestArguments(_)
        | Instruction::PushHole
        | Instruction::Yield
        | Instruction::SuperCall { .. }
        | Instruction::SuperApply
        | Instruction::SuperForward
        | Instruction::LoadNewTarget
        | Instruction::LoadArguments
        | Instruction::Await
        | Instruction::Pop
        | Instruction::Dup
        | Instruction::Unary(_)
        | Instruction::Binary(_)
        | Instruction::Jump(_)
        | Instruction::JumpIfFalse(_)
        | Instruction::JumpIfTruthy(_)
        | Instruction::JumpIfNotNullish(_)
        | Instruction::HostCall { .. }
        | Instruction::Return => Ok(()),
    }
}

fn validate_module(module: &ModuleProgram) -> Result<usize, EmitError> {
    let max_stack = validate(&module.program)?;
    let local_count = u64::from(module.program.local_count);
    let mut initialized_slots = HashSet::with_capacity(module.imports.len());
    for (index, binding) in module.imports.iter().enumerate() {
        if u64::from(binding.local_slot) >= local_count {
            return Err(EmitError::InvalidProgram {
                message: format!(
                    "import {index} local slot {} is out of bounds (local count {})",
                    binding.local_slot, module.program.local_count
                ),
            });
        }
        if !initialized_slots.insert(binding.local_slot) {
            return Err(EmitError::InvalidProgram {
                message: format!(
                    "import {index} local slot {} is initialized more than once",
                    binding.local_slot
                ),
            });
        }
    }
    for (index, binding) in module.exports.iter().enumerate() {
        if u64::from(binding.local_slot) >= local_count {
            return Err(EmitError::InvalidProgram {
                message: format!(
                    "export {index} local slot {} is out of bounds (local count {})",
                    binding.local_slot, module.program.local_count
                ),
            });
        }
    }
    Ok(max_stack)
}

fn invalid(index: usize, message: impl Into<String>) -> EmitError {
    EmitError::InvalidProgram {
        message: format!("instruction {index}: {}", message.into()),
    }
}

fn stack_effect(
    instruction: &Instruction,
    height: usize,
    index: usize,
) -> Result<(usize, bool), EmitError> {
    let underflow = |needed: usize| {
        invalid(
            index,
            format!("stack needs {needed} value(s), but has {height}"),
        )
    };
    match instruction {
        // Calls are interpreter-only for now. The native backend emits one C function per
        // module with a flat local array and no call stack, so these have no lowering yet --
        // and refusing here is what keeps the two backends from silently disagreeing about
        // what a program means. `kiln run` works on these; `kiln run --native` says why not.
        Instruction::Const(_)
        | Instruction::LoadLocal(_)
        | Instruction::LoadCell(_)
        | Instruction::LoadUpvalue(_)
        | Instruction::LoadBuiltin(_)
        | Instruction::LoadGlobal(_)
        | Instruction::RestArguments(_)
        | Instruction::PushHole
        | Instruction::LoadArguments
        | Instruction::MakeFunction(_) => Ok((height + 1, false)),
        // A literal consumes what it is built from and leaves the container.
        Instruction::NewObject { count } => {
            let needed = usize::try_from(*count)
                .unwrap_or(usize::MAX)
                .saturating_mul(2);
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        Instruction::NewArray { count } => {
            let needed = usize::try_from(*count).unwrap_or(usize::MAX);
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The object is replaced by what was read; the key and value are consumed with it.
        Instruction::GetProperty(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        Instruction::GetComputed => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // A write leaves the value it wrote, which is what makes `a = b.c = d` work.
        Instruction::SetProperty(_) | Instruction::SetPropertyStrict { .. } => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        Instruction::SetComputed | Instruction::SetComputedStrict { .. } => {
            if height < 3 {
                Err(underflow(3))
            } else {
                Ok((height - 2, false))
            }
        }
        Instruction::LoadThis => Ok((height + 1, false)),
        // The parent initialises the *existing* instance rather than allocating one, but it is
        // still a call and still leaves what it returned -- which the lowering pops, because
        // `super()` is a statement.
        Instruction::SuperCall { argc } => {
            let needed = usize::from(*argc) + 1;
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The arguments come from the frame rather than the stack, so only the parent is here
        // and the result replaces it.
        Instruction::SuperForward => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // `new` consumes the callee and its arguments and leaves the instance, exactly as a
        // call does.
        Instruction::New { argc } => {
            let needed = usize::from(*argc) + 1;
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The half and the key are consumed; the target stays, so a class body can define
        // several accessors on one object without reloading it.
        Instruction::DefineAccessor { .. } | Instruction::DefineMethod => {
            if height < 3 {
                Err(underflow(3))
            } else {
                Ok((height - 2, false))
            }
        }
        // Same shape with the key in the operand rather than on the stack: the function is
        // consumed and the target stays.
        Instruction::DefinePrivate { .. } => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // Replaces the value with an array of its own enumerable keys.
        Instruction::EnumerateKeys
        | Instruction::DeleteProperty(_)
        | Instruction::DeletePropertyStrict { .. } => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        Instruction::DeleteComputed | Instruction::DeleteComputedStrict { .. } => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // `[this, callee, arguments] -> [result]`: the arity is the length of an array the
        // program computed, which is exactly what a spread means.
        Instruction::Apply => {
            if height < 3 {
                Err(underflow(3))
            } else {
                Ok((height - 2, false))
            }
        }
        // `super(...xs)`: the parent and the computed argument list come off, its result goes on.
        Instruction::SuperApply => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // Replaces the value with an array of what it yields.
        Instruction::IterableToArray => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // The source is consumed and the target stays, the same shape `SetPrototype` uses.
        Instruction::SpreadObject | Instruction::SpreadInto => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // The prototype is written into the target, which stays.
        Instruction::SetPrototype => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // The property replaces the prototype it was read from.
        Instruction::LoadSuperProperty(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // The value replaces both the prototype and the key that were read from.
        Instruction::LoadSuperElement => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // The link replaces the value it was read from.
        Instruction::LoadPrototype => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // The receiver sits under the arguments, exactly where a callee would, and the result
        // replaces both -- so a method call and a plain call have the same shape here.
        Instruction::CallMethod { argc, .. } => {
            let needed = usize::from(*argc) + 1;
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The callee sits under its arguments, and the result replaces both.
        Instruction::Call { argc } => {
            let needed = usize::from(*argc) + 1;
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // Refused by `validate_supported_instruction`, so nothing is ever emitted for it. The
        // arithmetic is still written out rather than waved away: `stack_effect` is what proves
        // the height claims of a program consistent, and a lie here would be a lie about a
        // program the interpreter runs happily. `argc` is `None` when a spread put the arguments
        // in one array, which leaves exactly one value to consume.
        Instruction::DirectEval(site) => {
            let needed = usize::from(site.argc.unwrap_or(1));
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // Refused by `validate_supported_instruction` as well; the specifier becomes the
        // promise, so the height is unchanged. `with`'s two are the same shape: each replaces
        // the value on top with one derived from it.
        Instruction::DynamicImport | Instruction::ToObject | Instruction::WithHas(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // Both write a slot in place and touch no stack.
        Instruction::NewCell(_) | Instruction::MarkUninitialized(_) => Ok((height, false)),
        // Registering and retiring a handler leave the stack alone; the *target* is reached with
        // one more value on it, which the dataflow walk below accounts for.
        Instruction::PushHandler(_) | Instruction::PopHandler => Ok((height, false)),
        // A yield consumes the value and leaves one behind: what the caller sends back becomes
        // the result of the `yield` expression. `await` is identical in the machine -- the two
        // differ only in what the driver does with the value, which is not the body's business.
        Instruction::Yield | Instruction::Await => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // The name is a constant rather than a stack value. Control does *not* necessarily leave:
        // the instruction stands in for a value load, and the emitted runtime resolves the name
        // against the realm's global object before throwing -- so on the found path it pushes
        // exactly what `LoadBuiltin` would have, and the instructions after it were compiled
        // expecting that value. Modelling it as terminal would leave those instructions verified
        // at a height one short of the one the machine actually runs them at.
        Instruction::ThrowReference(_) => Ok((height + 1, false)),
        // A sloppy assignment to an unbound name. The value is consumed and the property lands on
        // the global object, so nothing is left behind: the compound and expression-valued forms
        // duplicate the value themselves before the store.
        Instruction::StoreGlobalBinding(_) | Instruction::StoreGlobalBindingStrict(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height - 1, false))
            }
        }
        // `delete unboundName` takes nothing and leaves the boolean the global object's
        // `[[Delete]]` answered.
        Instruction::DeleteGlobalBinding(_) => Ok((height + 1, false)),
        // A declaration touches the global object and leaves the stack where it found it.
        Instruction::DeclareGlobalBinding { .. } => Ok((height, false)),
        // A check, not a value: it either throws or leaves the stack alone.
        Instruction::RequireGlobalBinding(_) => Ok((height, false)),
        // A read that cannot fail: the value, or `undefined` when the realm has no such name.
        Instruction::LoadGlobalBindingOrUndefined(_) => Ok((height + 1, false)),
        // A literal produces a fresh object, so it only ever grows the stack.
        Instruction::NewRegExp { .. } => Ok((height + 1, false)),
        Instruction::LoadNewTarget => Ok((height + 1, false)),
        Instruction::Dup2 => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height + 2, false))
            }
        }
        // The receiver, the key and the arguments are all consumed and the result replaces them.
        Instruction::CallComputed { argc } => {
            let needed = usize::from(*argc).saturating_add(2);
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The callee and the argument list are both consumed and the instance replaces them.
        Instruction::ConstructSpread => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // The object is replaced by what was read, exactly as `GetProperty` is.
        Instruction::GetPrivate(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // ...and the value replaces both, exactly as `SetProperty` does: an assignment is an
        // expression and its value is what was assigned.
        Instruction::SetPrivate(_) => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // A throw consumes the value and does not fall through: control leaves for a handler.
        Instruction::Throw => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height - 1, true))
            }
        }
        Instruction::StoreLocal(_)
        | Instruction::StoreCell(_)
        | Instruction::StoreUpvalue(_)
        | Instruction::StoreGlobal(_)
        | Instruction::Pop => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height - 1, false))
            }
        }
        Instruction::Dup | Instruction::Unary(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((
                    height + usize::from(matches!(instruction, Instruction::Dup)),
                    false,
                ))
            }
        }
        Instruction::Binary(_) => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        Instruction::Jump(_) => Ok((height, false)),
        Instruction::JumpIfFalse(_)
        | Instruction::JumpIfTruthy(_)
        | Instruction::JumpIfNotNullish(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height - 1, false))
            }
        }
        Instruction::HostCall { argc, .. } => {
            let argc = usize::from(*argc);
            if height < argc {
                Err(underflow(argc))
            } else {
                Ok((height - argc + 1, false))
            }
        }
        // A return takes the top of the stack and abandons the frame, so anything under it is
        // simply gone. Residue is not a malformed program: `try { return 1 } finally { return 2 }`
        // reaches the second return with the first one's value still pending underneath, and the
        // interpreter has always truncated rather than complained. Requiring an empty stack here
        // rejected that shape outright, which is a refusal the language does not licence.
        Instruction::Return => Ok((height, true)),
    }
}

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

/// The local slots a body puts into the temporal dead zone.
fn dead_zone_slots(code: &[Instruction]) -> HashSet<u32> {
    code.iter()
        .filter_map(|instruction| match instruction {
            Instruction::MarkUninitialized(local) => Some(*local),
            _ => None,
        })
        .collect()
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
    for index in 0..program.functions.len() {
        writeln!(
            output,
            "static int {symbol}_fn_{index}(kiln_host *host, kiln_value *result, kiln_value *upvalues, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown, kiln_generator *frame);"
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

fn render_function(
    output: &mut String,
    program: &Program,
    proto: &FunctionProto,
    frame_stacks: &[usize],
    index: usize,
    symbol: &str,
    diagnostic_site: &str,
    strict_functions: &[bool],
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
        writeln!(
            output,
            "    for (slot = 0; slot < (size_t){locals}; ++slot) locals[slot] = kiln_c99_undefined();"
        )
        .unwrap();
        if proto.parameter_count == 0 {
            output.push_str("    (void)args; (void)argc;\n");
        } else {
            // Parameters occupy the first slots in declaration order. An argument that was not
            // passed leaves its slot undefined, which is what a missing argument is.
            writeln!(
                output,
                "    for (slot = 0; slot < (size_t){} && slot < argc; ++slot) locals[slot] = args[slot];",
                proto.parameter_count
            )
            .unwrap();
        }
    }
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
        guarded: dead_zone_slots(&proto.code),
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
    let mut output = String::with_capacity(HEADER.len() + program.code.len() * 64);
    render_emission_policy(&mut output, policy);
    output.push_str(HEADER);
    output = output.replace(
        "/* KILN_NATIVE_CONSTRUCTOR_TABLE */",
        &render_native_constructor_table(),
    );
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
        guarded: dead_zone_slots(&program.code),
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
    let mut output = String::with_capacity(HEADER.len() + program.code.len() * 72);
    render_emission_policy(&mut output, policy);
    output.push_str(HEADER);
    output = output.replace(
        "/* KILN_NATIVE_CONSTRUCTOR_TABLE */",
        &render_native_constructor_table(),
    );
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
        guarded: dead_zone_slots(&program.code),
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
    /// The local slots this body puts into the temporal dead zone, so a read of one is checked
    /// and every other read is not.
    guarded: HashSet<u32>,
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
                writeln!(output, "    stack[sp++] = kiln_c99_boolean({});", i32::from(*value)).unwrap()
            }
            Constant::Number(bits) => {
                writeln!(output, "    stack[sp++] = kiln_c99_number_bits(UINT64_C({bits}));").unwrap()
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
            if guarded.contains(local) {
                let unwind = unwind_or_return(handlers, site);
                writeln!(
                    output,
                    "    if (kiln_c99_cell_of(locals[{local}])->tag == KILN_VALUE_UNINITIALIZED) {{ int status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, \"cannot access a binding before its declaration\"); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
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
            writeln!(output, "    if (locals[{local}].tag == KILN_VALUE_CELL) {{ *kiln_c99_cell_of(locals[{local}]) = kiln_c99_uninitialized(); }} else {{ locals[{local}] = kiln_c99_uninitialized(); }}").unwrap();
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
            // The fast path below skips `kiln_c99_call` and invokes the body directly, which is
            // only sound while it can supply the same receiver the dispatcher would have. It
            // supplies two: an arrow's captured `this`, and `undefined` for everything else.
            //
            // `undefined` stopped being the whole of the second case when the dispatcher learned
            // `OrdinaryCallBindThis` -- sloppy code never sees `undefined` as `this`, it sees the
            // global object -- so a sloppy non-arrow function taking this path got `undefined`
            // where the interpreter gave it `globalThis`, and a UMD header's `this || global`
            // silently picked the wrong branch. Neither side was written wrong: the fast path
            // replicated a tail that later grew a rule, and the two only met at a merge.
            //
            // So the guard names the condition rather than the rule: take the fast path only when
            // the receiver it already knows how to supply is the right one. The substitution
            // itself stays in `kiln_c99_call` and stays written once, because a rule with two
            // implementations is what drifted here in the first place. Modules are strict, so
            // this costs nothing outside CommonJS.
            writeln!(
                output,
                "    {{ kiln_value call_result = kiln_c99_undefined(); kiln_value callee = stack[sp - {argc} - 1]; int status; kiln_value call_argbuf[({argc}) + 1]; for (size_t i = 0; i < (size_t){argc}; i++) {{ call_argbuf[i] = stack[sp - {argc} + i]; }} const kiln_value *call_args = {argc} == 0 ? NULL : call_argbuf; {{ kiln_closure *plain = callee.tag == KILN_VALUE_FUNCTION ? kiln_c99_closure_of(callee) : NULL; if (plain != NULL && (plain->arrow || plain->strict) && !plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ status = kiln_c99_invoke_closure(host, &call_result, plain, plain->arrow ? plain->captured_this : kiln_c99_undefined(), call_args, (size_t){argc}, depth, thrown, NULL, 0); }} else {{ status = kiln_c99_call(host, &call_result, callee, kiln_c99_undefined(), call_args, (size_t){argc}, depth, thrown); }} }} if (status == KILN_C99_CONTROL_ERROR && call_result.tag == KILN_VALUE_STRING) {{ *result = call_result; return status; }} if (status == KILN_C99_CONTROL_ERROR && kiln_c99_diagnostic_site == NULL && kiln_c99_diagnostic_operation == NULL && kiln_c99_diagnostic_detail == NULL) {{ kiln_c99_diagnostic_receiver_tag = callee.tag; kiln_c99_diagnostic_detail = \"Call callee is unsupported by the native callable model\"; }} if (status == KILN_C99_CONTROL_ERROR) kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = call_result; }}",
                argc + 1
            )
            .unwrap();
        }
        Instruction::LoadLocal(local) => {
            if guarded.contains(local) {
                let unwind = unwind_or_return(handlers, site);
                writeln!(
                    output,
                    "    if (locals[{local}].tag == KILN_VALUE_UNINITIALIZED) {{ int status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, \"cannot access a binding before its declaration\"); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
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
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; kiln_value read; kiln_value target = stack[sp - 1]; int status = kiln_c99_get_site(host, target, {symbol}_key_{name}, {}, &read, depth, thrown, &site); if (status == KILN_C99_CONTROL_ERROR && kiln_c99_diagnostic_detail == NULL) {{ kiln_c99_diagnostic_receiver_tag = target.tag; kiln_c99_diagnostic_detail = \"GetProperty lookup refused\"; }} if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = read; }}",
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
                "    {{ kiln_value key = stack[--sp]; kiln_value read; int status = kiln_c99_get_computed(host, stack[sp - 1], key, &read, depth, thrown); if (status == KILN_C99_CONTROL_ERROR && kiln_c99_diagnostic_detail == NULL) {{ kiln_c99_diagnostic_receiver_tag = stack[sp - 1].tag; kiln_c99_diagnostic_detail = \"GetComputed lookup refused\"; }} if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = read; }}"
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
            if guarded.contains(slot) {
                let unwind = unwind_or_return(handlers, site);
                writeln!(
                    output,
                    "    if ({symbol}_globals[{slot}].tag == KILN_VALUE_UNINITIALIZED) {{ int status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, \"cannot access a binding before its declaration\"); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
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
                "    {{ kiln_value built; int status; kiln_value call_argbuf[({argc}) + 1]; for (size_t i = 0; i < (size_t){argc}; i++) {{ call_argbuf[i] = stack[sp - {argc} + i]; }} const kiln_value *call_args = {argc} == 0 ? NULL : call_argbuf; status = kiln_c99_construct(host, &built, stack[sp - {argc} - 1], call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_CONTROL_ERROR && kiln_c99_diagnostic_detail == NULL) {{ kiln_c99_diagnostic_receiver_tag = stack[sp - {argc} - 1].tag; if (stack[sp - {argc} - 1].tag == KILN_VALUE_BUILTIN) {{ const char *named = kiln_c99_builtin_name(stack[sp - {argc} - 1].bits); if (named != NULL) {{ kiln_c99_diagnostic_function = named; kiln_c99_diagnostic_function_len = strlen(named); }} }} kiln_c99_diagnostic_detail = \"New target is unsupported by the native constructor model\"; }} if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = built; }}",
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
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; kiln_value method; kiln_value call_result = kiln_c99_undefined(); int status; kiln_value self_value = stack[sp - {argc} - 1]; kiln_value call_argbuf[({argc}) + 1]; for (size_t i = 0; i < (size_t){argc}; i++) {{ call_argbuf[i] = stack[sp - {argc} + i]; }} const kiln_value *call_args = {argc} == 0 ? NULL : call_argbuf; status = kiln_c99_get_site(host, self_value, {symbol}_key_{name}, {}, &method, depth, thrown, &site); if (status == KILN_C99_CONTROL_ERROR && kiln_c99_diagnostic_detail == NULL) {{ kiln_c99_diagnostic_receiver_tag = self_value.tag; kiln_c99_diagnostic_detail = \"CallMethod property lookup refused\"; }} if (status == KILN_C99_CONTROL_ERROR) kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); if (status == KILN_C99_THROWN) {{ if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = \"CallMethod property lookup threw\"; kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); {unwind} }} if (status != KILN_C99_OK) return status; status = kiln_c99_call(host, &call_result, method, self_value, call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_CONTROL_ERROR && call_result.tag == KILN_VALUE_STRING) {{ *result = call_result; return status; }} if (status == KILN_C99_CONTROL_ERROR && kiln_c99_diagnostic_detail == NULL) {{ kiln_c99_diagnostic_receiver_tag = method.tag; kiln_c99_diagnostic_detail = \"CallMethod invocation refused\"; }} if (status == KILN_C99_CONTROL_ERROR) kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); if (status == KILN_C99_THROWN) {{ if (kiln_c99_diagnostic_detail == NULL) {{ kiln_c99_diagnostic_receiver_tag = method.tag; kiln_c99_diagnostic_detail = \"CallMethod invocation threw\"; }} kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); {unwind} }} if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = call_result; }}",
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
            let unwind = unwind_or_return(handlers, site);
            match numeric_fast_unary_body(*operator) {
                Some(fast) => writeln!(output, "    {{ kiln_value operand = stack[sp - 1]; kiln_value answer; if (operand.tag == KILN_VALUE_NUMBER) {{ double a; memcpy(&a, &operand.bits, sizeof(a)); {fast} }} else {{ int status = kiln_c99_unary_checked({}, host, operand, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }} stack[sp - 1] = answer; }}", unary_code(*operator)).unwrap(),
                None => writeln!(output, "    {{ kiln_value answer; int status = kiln_c99_unary_checked({}, host, stack[sp - 1], &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = answer; }}", unary_code(*operator)).unwrap(),
            }
        }
        Instruction::Binary(operator) if !*callables => {
            // A scalar program has no BigInt in it and so no operator in it can throw. It also
            // has nowhere to put a thrown value -- the entry point declares that slot only when
            // something can reach it -- so this is the form that program has to keep.
            match numeric_fast_body(*operator) {
                Some(fast) => writeln!(output, "    {{ kiln_value right = stack[--sp]; kiln_value left = stack[--sp]; kiln_value answer; if (left.tag == KILN_VALUE_NUMBER && right.tag == KILN_VALUE_NUMBER) {{ double a; double b; memcpy(&a, &left.bits, sizeof(a)); memcpy(&b, &right.bits, sizeof(b)); {fast} }} else {{ answer = kiln_c99_binary({}, left, right); }} stack[sp++] = answer; }}", binary_code(*operator)).unwrap(),
                None => writeln!(output, "    {{ kiln_value right = stack[--sp]; kiln_value left = stack[--sp]; stack[sp++] = kiln_c99_binary({}, left, right); }}", binary_code(*operator)).unwrap(),
            }
        }
        Instruction::Binary(operator) => {
            // An operator throws only when a BigInt is involved, and `kiln_c99_binary_checked`
            // hands every other case straight back to `kiln_c99_binary`.
            let unwind = unwind_or_return(handlers, site);
            match numeric_fast_body(*operator) {
                Some(fast) => writeln!(output, "    {{ kiln_value right = stack[--sp]; kiln_value left = stack[--sp]; kiln_value answer; if (left.tag == KILN_VALUE_NUMBER && right.tag == KILN_VALUE_NUMBER) {{ double a; double b; memcpy(&a, &left.bits, sizeof(a)); memcpy(&b, &right.bits, sizeof(b)); {fast} }} else {{ int status = kiln_c99_binary_checked({}, host, left, right, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }} stack[sp++] = answer; }}", binary_code(*operator)).unwrap(),
                None => writeln!(output, "    {{ kiln_value right = stack[--sp]; kiln_value left = stack[--sp]; kiln_value answer; int status = kiln_c99_binary_checked({}, host, left, right, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp++] = answer; }}", binary_code(*operator)).unwrap(),
            }
        }
        Instruction::Jump(target) => writeln!(output, "    goto kiln_label_{target};").unwrap(),
        Instruction::JumpIfFalse(target) => {
            writeln!(output, "    if (!kiln_c99_truthy(stack[--sp])) goto kiln_label_{target};").unwrap()
        }
        Instruction::JumpIfTruthy(target) => {
            writeln!(output, "    if (kiln_c99_truthy(stack[--sp])) goto kiln_label_{target};").unwrap()
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
                    output.push_str(
                        "    { kiln_value module_value = locals[2].tag == KILN_VALUE_CELL ? *kiln_c99_cell_of(locals[2]) : locals[2]; int export_status = kiln_c99_get(host, module_value, \"exports\", 7, &exports[0], depth, thrown); if (export_status != KILN_C99_OK) return export_status; }\n",
                    );
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
    operation
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric() || b"_.$:-()".contains(byte))
        .map(char::from)
        .collect()
}

/// [`c_string`], broken into adjacent literals so no single one grows unbounded.
///
/// C99 only requires a compiler to accept a 4095-character string literal, and a unit's whole
/// source is routinely far past that -- a bundle is megabytes. Adjacent string literals are
/// concatenated by the translation phase that runs before the limit applies, so splitting costs
/// nothing at run time and keeps the emitted C inside what the standard guarantees.
fn c_string_chunked(value: &str) -> String {
    const CHUNK: usize = 2048;
    let mut output = String::from("\"");
    let mut since = 0usize;
    for byte in value.bytes() {
        let before = output.len();
        push_c_byte(&mut output, byte);
        since += output.len() - before;
        if since >= CHUNK {
            output.push_str("\"\n\"");
            since = 0;
        }
    }
    output.push('"');
    output
}

/// One source byte as it is spelled inside a C string literal.
fn push_c_byte(output: &mut String, byte: u8) {
    match byte {
        b'"' => output.push_str("\\\""),
        b'\\' => output.push_str("\\\\"),
        b'\n' => output.push_str("\\n"),
        b'\r' => output.push_str("\\r"),
        b'\t' => output.push_str("\\t"),
        0x20..=0x7e => output.push(char::from(byte)),
        // The empty literal that follows ends the hex escape, which would otherwise swallow any
        // hex digit written next to it.
        _ => write!(output, "\\x{byte:02X}\"\"").unwrap(),
    }
}

fn c_string(value: &str) -> String {
    let mut output = String::from("\"");
    for byte in value.bytes() {
        push_c_byte(&mut output, byte);
    }
    output.push('"');
    output
}

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
        BinaryOp::Add => "answer = kiln_c99_number(a + b);",
        BinaryOp::Subtract => "answer = kiln_c99_number(a - b);",
        BinaryOp::Multiply => "answer = kiln_c99_number(a * b);",
        BinaryOp::Divide => "answer = kiln_c99_number(a / b);",
        // `%` is `fmod` by specification, and `fmod` is a call into libm that measured at 41% of a
        // Collatz benchmark's entire runtime. Two integral operands take the machine instruction
        // instead, which agrees exactly: C truncates toward zero and takes the sign of the
        // dividend, as ECMAScript specifies. The guard stays conservative — outside +/-2^53 a
        // double is not exactly an integer, a zero divisor must reach `fmod` for NaN, and a zero
        // dividend must too, because `-0 % 3` is `-0` and integer division would answer `+0`.
        BinaryOp::Remainder => {
            "if (a != 0.0 && b != 0.0 && a >= -9007199254740992.0 && a <= 9007199254740992.0 &&              b >= -9007199254740992.0 && b <= 9007199254740992.0 && a == (double)(int64_t)a &&              b == (double)(int64_t)b) { answer = kiln_c99_number((double)((int64_t)a % (int64_t)b)); }              else { answer = kiln_c99_number(fmod(a, b)); }"
        }
        BinaryOp::StrictEqual => "answer = kiln_c99_boolean(a == b);",
        BinaryOp::StrictNotEqual => "answer = kiln_c99_boolean(a != b);",
        BinaryOp::LessThan => "answer = kiln_c99_boolean(a < b);",
        BinaryOp::LessThanOrEqual => "answer = kiln_c99_boolean(a <= b);",
        BinaryOp::GreaterThan => "answer = kiln_c99_boolean(a > b);",
        BinaryOp::GreaterThanOrEqual => "answer = kiln_c99_boolean(a >= b);",
        _ => return None,
    })
}

/// The unary counterpart of [`numeric_fast_body`], answering in terms of `a`.
///
/// `typeof` and `void` do not touch the value and `~` needs ToInt32, so they keep the generic call.
fn numeric_fast_unary_body(operator: UnaryOp) -> Option<&'static str> {
    Some(match operator {
        UnaryOp::Plus | UnaryOp::ToNumeric => "answer = kiln_c99_number(a);",
        UnaryOp::Negate => "answer = kiln_c99_number(-a);",
        // `!x` on a number is false unless the number is zero or NaN.
        UnaryOp::Not => "answer = kiln_c99_boolean(a == 0.0 || a != a);",
        UnaryOp::Increment => "answer = kiln_c99_number(a + 1.0);",
        UnaryOp::Decrement => "answer = kiln_c99_number(a - 1.0);",
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
    for value in [0x70, u8::from(policy.host_clock), entropy_tag] {
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
    for value in [0x71, u8::from(policy.host_clock), entropy_tag] {
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
mod tests {
    use super::*;
    use kiln_ir::Builtin;
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    #[test]
    fn default_emission_policy_is_inert_and_host_clock_changes_identity() {
        let program = Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![Instruction::Return],
            local_count: 0,
            functions: Vec::new(),
        };
        let legacy = emit(&program).expect("default program emits");
        let explicit_default =
            emit_with_policy(&program, EmissionPolicy::default()).expect("default policy emits");
        let host_clock = emit_with_policy(
            &program,
            EmissionPolicy {
                host_clock: true,
                ..EmissionPolicy::default()
            },
        )
        .expect("host-clock policy emits");
        let seeded = emit_with_policy(
            &program,
            EmissionPolicy {
                entropy: EmissionEntropyPolicy::Seeded(7),
                ..EmissionPolicy::default()
            },
        )
        .expect("seeded policy emits");
        let host_entropy = emit_with_policy(
            &program,
            EmissionPolicy {
                entropy: EmissionEntropyPolicy::Host,
                ..EmissionPolicy::default()
            },
        )
        .expect("host entropy policy emits");
        let named_timezone = emit_with_policy(
            &program,
            EmissionPolicy {
                timezone: EmissionStringPolicy::Named("America/Denver".into()),
                ..EmissionPolicy::default()
            },
        )
        .expect("named timezone policy emits");
        let host_timezone = emit_with_policy(
            &program,
            EmissionPolicy {
                timezone: EmissionStringPolicy::Host,
                ..EmissionPolicy::default()
            },
        )
        .expect("host timezone policy emits");
        let named_locale = emit_with_policy(
            &program,
            EmissionPolicy {
                locale: EmissionStringPolicy::Named("de-DE".into()),
                ..EmissionPolicy::default()
            },
        )
        .expect("named locale policy emits");
        let host_locale = emit_with_policy(
            &program,
            EmissionPolicy {
                locale: EmissionStringPolicy::Host,
                ..EmissionPolicy::default()
            },
        )
        .expect("host locale policy emits");

        assert_eq!(legacy, explicit_default);
        assert_ne!(legacy.symbol, host_clock.symbol);
        assert_ne!(legacy.program_hash, host_clock.program_hash);
        assert!(!legacy.source.contains("#define KILN_REALM_HOST_CLOCK"));
        assert!(
            host_clock
                .source
                .contains("#define KILN_REALM_HOST_CLOCK 1")
        );
        assert!(host_clock.source.contains("UINT16_C(2)"));
        assert_ne!(legacy.symbol, seeded.symbol);
        assert_ne!(seeded.symbol, host_entropy.symbol);
        assert!(
            seeded
                .source
                .contains("#define KILN_REALM_ENTROPY_SEED UINT64_C(7)")
        );
        assert!(
            host_entropy
                .source
                .contains("#define KILN_REALM_HOST_ENTROPY 1")
        );
        assert!(host_entropy.source.contains("UINT16_C(3)"));
        assert_ne!(legacy.symbol, named_timezone.symbol);
        assert_ne!(named_timezone.symbol, host_timezone.symbol);
        assert_ne!(legacy.symbol, named_locale.symbol);
        assert_ne!(named_locale.symbol, host_locale.symbol);
        assert_ne!(named_timezone.symbol, named_locale.symbol);
    }

    #[test]
    fn runtime_module_materializer_stays_inside_the_backend_value_abi() {
        let module = ModuleProgram {
            program: Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: Vec::new(),
                code: vec![Instruction::Return],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: vec![kiln_ir::ExportBinding {
                exported: "probe".to_owned(),
                local_slot: 0,
            }],
            strictness: kiln_ir::Strictness::default(),
        };
        let emitted = emit_runtime_module_with_logical_path(
            &module,
            ".kiln/builtins/probe.builtin",
            "exports[0] = kiln_c99_method(KILN_C99_METHOD_QUEUE_MICROTASK);",
        )
        .expect("runtime module emits");

        assert!(
            emitted
                .source
                .contains("exports[0] = kiln_c99_method(KILN_C99_METHOD_QUEUE_MICROTASK);")
        );
        assert!(emitted.source.contains("static int kiln_module_path_"));
        assert!(emitted.source.contains("kiln_realm_enter"));
        assert!(!emitted.source.contains("extern kiln_value"));
    }

    #[test]
    fn executable_namespace_wrapper_materializes_sorted_immutable_namespace() {
        let module = ModuleProgram {
            program: Program {
                version: BYTECODE_VERSION,
                source: String::new().into(),
                constants: Vec::new(),
                code: vec![Instruction::Return],
                local_count: 0,
                functions: Vec::new(),
            },
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            strictness: kiln_ir::Strictness::default(),
        };
        let generated = emit_module_with_logical_path(&module, "src/namespace.js")
            .expect("namespace helper module emits");
        let same = emit_module_with_logical_path(&module, "src/namespace.js")
            .expect("namespace helper module emits deterministically");
        assert_eq!(generated, same);
        let wrapper = format!(
            r#"
int main(void) {{
    static const char *names[] = {{"z", "a"}};
    static const size_t lengths[] = {{1, 1}};
    static const char *numeric_names[] = {{"2", "10"}};
    static const size_t numeric_lengths[] = {{1, 2}};
    static const char *reserved_names[] = {{"@@toStringTag"}};
    static const size_t reserved_lengths[] = {{13}};
    kiln_value values[2] = {{kiln_c99_number(2.0), kiln_c99_number(1.0)}};
    kiln_value namespace_object;
    kiln_value numeric_namespace;
    kiln_value empty_namespace;
    kiln_value cell = values[0];
    kiln_value read;
    kiln_value descriptor;
    kiln_value thrown = kiln_c99_undefined();
    kiln_value args[3];
    int found;
    if (kiln_c99_make_cell(&cell) != KILN_C99_OK) return 1;
    *kiln_c99_cell_of(cell) = kiln_c99_number(9.0);
    values[0] = cell;
    if ({symbol}_namespace_wrapper(&namespace_object, names, lengths, values, 2) != KILN_C99_OK)
        return 2;
    if (namespace_object.tag != KILN_VALUE_OBJECT) return 3;
    if (kiln_c99_object_of(namespace_object)->prototype.tag != KILN_VALUE_NULL) return 4;
    if (!kiln_c99_object_of(namespace_object)->module_namespace ||
        kiln_c99_object_of(namespace_object)->frozen) return 5;
    if (kiln_c99_object_of(namespace_object)->count != 3 ||
        strcmp(kiln_c99_object_of(namespace_object)->properties[0].key, "a") != 0 ||
        strcmp(kiln_c99_object_of(namespace_object)->properties[1].key, "z") != 0 ||
        strcmp(kiln_c99_object_of(namespace_object)->properties[2].key, "@@toStringTag") != 0)
        return 6;
    if (kiln_c99_get(NULL, namespace_object, "z", 1, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_NUMBER || kiln_c99_as_number(read) != 9.0) return 7;
    if (kiln_c99_describe_property(NULL, namespace_object, "a", 1, &descriptor, &found, 0,
                                   &thrown) != KILN_C99_OK || !found) return 8;
    if (kiln_c99_get(NULL, descriptor, "writable", 8, &read, 0, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(read)) return 9;
    if (kiln_c99_get(NULL, descriptor, "enumerable", 10, &read, 0, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(read)) return 10;
    if (kiln_c99_get(NULL, descriptor, "configurable", 12, &read, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_truthy(read)) return 11;
    args[0] = namespace_object;
    args[1] = kiln_c99_copy_text("a", 1);
    args[2] = descriptor;
    if (kiln_c99_invoke_method(NULL, &read, KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY,
                               kiln_c99_undefined(), args, 3, 0, &thrown) != KILN_C99_OK)
        return 12;
    if (kiln_c99_put(NULL, descriptor, "writable", 8, kiln_c99_boolean(0), 0, &thrown) !=
        KILN_C99_OK ||
        kiln_c99_invoke_method(NULL, &read, KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY,
                               kiln_c99_undefined(), args, 3, 0, &thrown) != KILN_C99_THROWN)
        return 13;
    if (kiln_c99_get(NULL, namespace_object, "@@toStringTag", 13, &read, 0, &thrown) !=
            KILN_C99_OK ||
        read.tag != KILN_VALUE_STRING || read.len != 6 || memcmp(read.ptr, "Module", 6) != 0)
        return 13;
    if (kiln_c99_put_mode(NULL, namespace_object, "a", 1, kiln_c99_number(3.0), 1, 0,
                          &thrown) != KILN_C99_THROWN)
        return 14;
    if (kiln_c99_get(NULL, thrown, "message", 7, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_STRING || strstr(read.ptr, "module namespace") == NULL) return 15;
    if (kiln_c99_write_would_accept(namespace_object, "a", 1)) return 16;
    if (kiln_c99_delete(NULL, namespace_object, "a", 1, &found, 0, &thrown) != KILN_C99_OK ||
        found != 0) return 17;
    if (kiln_c99_delete(NULL, namespace_object, "missing", 7, &found, 0, &thrown) !=
            KILN_C99_OK ||
        found != 1) return 18;
    args[0] = namespace_object;
    if (kiln_c99_invoke_method(NULL, &read, KILN_C99_METHOD_OBJECT_IS_FROZEN,
                               kiln_c99_undefined(), args, 1, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_truthy(read)) return 19;
    if (kiln_c99_invoke_method(NULL, &read, KILN_C99_METHOD_OBJECT_FREEZE,
                               kiln_c99_undefined(), args, 1, 0, &thrown) != KILN_C99_THROWN)
        return 20;
    if (kiln_c99_set_prototype(namespace_object, kiln_c99_undefined()) !=
        KILN_C99_CONTROL_ERROR) return 21;
    if ({symbol}_namespace_wrapper(&numeric_namespace, numeric_names, numeric_lengths, values, 2)
        != KILN_C99_OK) return 22;
    if (kiln_c99_object_of(numeric_namespace)->count != 3 ||
        strcmp(kiln_c99_object_of(numeric_namespace)->properties[0].key, "10") != 0 ||
        strcmp(kiln_c99_object_of(numeric_namespace)->properties[1].key, "2") != 0)
        return 23;
    if ({symbol}_namespace_wrapper(&read, reserved_names, reserved_lengths, values, 1) !=
        KILN_C99_INVALID_ARGUMENT) return 24;
    if ({symbol}_namespace_wrapper(&empty_namespace, NULL, NULL, NULL, 0) != KILN_C99_OK)
        return 25;
    args[0] = empty_namespace;
    if (kiln_c99_invoke_method(NULL, &read, KILN_C99_METHOD_OBJECT_FREEZE,
                               kiln_c99_undefined(), args, 1, 0, &thrown) != KILN_C99_OK ||
        !kiln_c99_strict_equal(read, empty_namespace)) return 26;
    if (kiln_c99_invoke_method(NULL, &read, KILN_C99_METHOD_OBJECT_IS_FROZEN,
                               kiln_c99_undefined(), args, 1, 0, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(read)) return 27;
    return 0;
}}
"#,
            symbol = generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "namespace-wrapper");
        assert!(
            run.status.success(),
            "generated namespace wrapper failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_runtime_event_emitter_preserves_order_once_and_removal() {
        let module = ModuleProgram {
            program: Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: Vec::new(),
                code: vec![Instruction::Return],
                local_count: 1,
                functions: Vec::new(),
            },
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: vec![kiln_ir::ExportBinding {
                exported: "EventEmitter".to_owned(),
                local_slot: 0,
            }],
            strictness: kiln_ir::Strictness::default(),
        };
        let generated = emit_runtime_module_with_logical_path(
            &module,
            ".kiln/builtins/events.builtin",
            "exports[0] = kiln_c99_method(KILN_C99_METHOD_NODE_EVENT_EMITTER);",
        )
        .expect("events runtime module emits");
        let wrapper = format!(
            r#"
static int calls = 0;
static int listener(kiln_host *host, kiln_value *result, kiln_value *upvalues,
                    kiln_value receiver, const kiln_value *args, size_t argc, int depth,
                    kiln_value *thrown, kiln_generator *frame) {{
    (void)host; (void)upvalues; (void)receiver; (void)depth; (void)thrown; (void)frame;
    if (argc != 1 || args[0].tag != KILN_VALUE_NUMBER) return 91;
    calls = calls * 10 + (int)kiln_c99_as_number(args[0]);
    *result = kiln_c99_undefined();
    return KILN_C99_OK;
}}
static int child_constructor(kiln_host *host, kiln_value *result, kiln_value *upvalues,
                             kiln_value receiver, const kiln_value *args, size_t argc, int depth,
                             kiln_value *thrown, kiln_generator *frame) {{
    (void)host; (void)upvalues; (void)args; (void)argc; (void)depth; (void)thrown; (void)frame;
    *result = receiver;
    return KILN_C99_OK;
}}
int main(void) {{
    uint64_t realm;
    kiln_value exports[1];
    kiln_value result;
    kiln_value emitter;
    kiln_value callback;
    kiln_value child;
    kiln_value child_instance;
    kiln_value child_prototype;
    kiln_value parent_prototype;
    kiln_value inherited;
    kiln_value args[2];
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_make_closure(&callback, listener, "listener", 8, NULL, 0, "<test>", 0, 1, 0, 0, 0, 0,
                              1, kiln_c99_undefined(), kiln_c99_undefined(), 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK) return 1;
    if ({symbol}(NULL, &result, NULL, 0, exports, 1) != KILN_C99_OK) return 2;
    if (kiln_c99_construct(NULL, &emitter, exports[0], NULL, 0, 0,
                           &thrown) != KILN_C99_OK) return 3;
    if (emitter.tag != KILN_VALUE_OBJECT) return 31;
    {{ kiln_value keys;
       if (kiln_c99_own_keys(NULL, emitter, &keys, 0, &thrown) != KILN_C99_OK) return 32;
       if (kiln_c99_array_of(keys)->count != 0) return 33;
    }}
    if (kiln_c99_make_closure(&child, child_constructor, "Child", 5, NULL, 0, "<test>", 0, 0, 0, 0, 0, 0,
                              1, kiln_c99_undefined(), kiln_c99_undefined(), 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK) return 34;
    args[0] = child; args[1] = exports[0];
    if (kiln_c99_node_util(NULL, &result, KILN_C99_METHOD_NODE_UTIL_INHERITS, args, 2, 0,
                           &thrown) != KILN_C99_OK) return 35;
    if (kiln_c99_construct(NULL, &child_instance, child, NULL, 0, 0,
                           &thrown) != KILN_C99_OK) return 36;
    if (kiln_c99_get(NULL, child_instance, "on", 2, &inherited, 0,
                     &thrown) != KILN_C99_OK ||
        inherited.tag != KILN_VALUE_METHOD ||
        inherited.bits != KILN_C99_METHOD_NODE_EVENT_ON) return 37;
    if (kiln_c99_callable_prototype(child, &child_prototype) != KILN_C99_OK ||
        kiln_c99_callable_prototype(exports[0], &parent_prototype) != KILN_C99_OK ||
        kiln_c99_object_of(child_prototype)->prototype.bits != parent_prototype.bits) return 38;
    args[0] = kiln_c99_copy_text("tick", 4); args[1] = callback;
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_NODE_EVENT_ON, emitter, args, 2,
                               0, &thrown) != KILN_C99_OK) return 4;
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_NODE_EVENT_ONCE, emitter, args, 2,
                               0, &thrown) != KILN_C99_OK) return 5;
    args[1] = kiln_c99_number(7.0);
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_NODE_EVENT_EMIT, emitter, args, 2,
                               0, &thrown) != KILN_C99_OK || !kiln_c99_truthy(result)) return 6;
    args[1] = kiln_c99_number(8.0);
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_NODE_EVENT_EMIT, emitter, args, 2,
                               0, &thrown) != KILN_C99_OK || calls != 778) return 7;
    args[1] = callback;
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_NODE_EVENT_REMOVE_LISTENER,
                               emitter, args, 2, 0, &thrown) != KILN_C99_OK) return 8;
    args[1] = kiln_c99_number(9.0);
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_NODE_EVENT_EMIT, emitter, args, 2,
                               0, &thrown) != KILN_C99_OK || kiln_c99_truthy(result)) return 9;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 39;
}}
"#,
            symbol = generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "runtime-event-emitter");
        assert!(
            run.status.success(),
            "runtime EventEmitter failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_runtime_readable_has_a_checked_native_prototype_graph() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("empty program emits runtime prelude");
        let wrapper = r#"
static int readable_child(kiln_host *host, kiln_value *result, kiln_value *upvalues,
                          kiln_value receiver, const kiln_value *args, size_t argc, int depth,
                          kiln_value *thrown, kiln_generator *frame) {
    (void)host; (void)upvalues; (void)args; (void)argc; (void)depth; (void)thrown; (void)frame;
    *result = receiver;
    return KILN_C99_OK;
}
int main(void) {
    uint64_t realm;
    kiln_value readable = kiln_c99_method(KILN_C99_METHOD_NODE_READABLE);
    kiln_value child;
    kiln_value result;
    kiln_value instance;
    kiln_value method;
    kiln_value readable_prototype;
    kiln_value emitter_prototype;
    kiln_value arguments[2];
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_callable_prototype(readable, &readable_prototype) != KILN_C99_OK) return 1;
    if (kiln_c99_callable_prototype(kiln_c99_method(KILN_C99_METHOD_NODE_EVENT_EMITTER),
                                    &emitter_prototype) != KILN_C99_OK) return 2;
    if (kiln_c99_object_of(readable_prototype)->prototype.bits != emitter_prototype.bits) return 3;
    if (kiln_c99_get(NULL, readable_prototype, "push", 4, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD ||
        method.bits != KILN_C99_METHOD_NODE_READABLE_PUSH) return 4;
    if (kiln_c99_get(NULL, readable_prototype, "on", 2, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD || method.bits != KILN_C99_METHOD_NODE_EVENT_ON) return 5;
    if (kiln_c99_make_closure(&child, readable_child, "Child", 5, "<test>", 6, "<test>", 0, 0, 0, 0, 0, 0,
                              1, kiln_c99_undefined(), kiln_c99_undefined(), 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK) return 6;
    arguments[0] = child;
    arguments[1] = readable;
    if (kiln_c99_node_util(NULL, &result, KILN_C99_METHOD_NODE_UTIL_INHERITS, arguments, 2, 0,
                           &thrown) != KILN_C99_OK) return 7;
    if (kiln_c99_construct(NULL, &instance, child, NULL, 0, 0, &thrown) != KILN_C99_OK) return 8;
    if (kiln_c99_get(NULL, instance, "push", 4, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD ||
        method.bits != KILN_C99_METHOD_NODE_READABLE_PUSH) return 9;
    if (kiln_c99_get(NULL, instance, "on", 2, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD || method.bits != KILN_C99_METHOD_NODE_EVENT_ON) return 10;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 11;
}
"#;
        let run = run_with_main(&generated, wrapper, "native-readable-prototype-graph");
        assert!(
            run.status.success(),
            "native Readable prototype graph failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_server_response_inherits_writable_through_the_native_graph() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("empty program emits runtime prelude");
        let wrapper = r#"
int main(void) {
    kiln_value constructor = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE);
    kiln_value prototype;
    kiln_value writable_prototype;
    kiln_value instance;
    kiln_value method;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_c99_callable_prototype(constructor, &prototype) != KILN_C99_OK) return 1;
    if (kiln_c99_callable_prototype(kiln_c99_method(KILN_C99_METHOD_NODE_WRITABLE),
                                    &writable_prototype) != KILN_C99_OK) return 2;
    if (kiln_c99_object_of(prototype)->prototype.bits != writable_prototype.bits) return 3;
    if (kiln_c99_get(NULL, prototype, "writeHead", 9, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD ||
        method.bits != KILN_C99_METHOD_NODE_RESPONSE_WRITE_HEAD) return 4;
    if (kiln_c99_get(NULL, prototype, "on", 2, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD || method.bits != KILN_C99_METHOD_NODE_EVENT_ON) return 5;
    if (kiln_c99_construct(NULL, &instance, constructor, NULL, 0, 0, &thrown) != KILN_C99_OK)
        return 6;
    if (kiln_c99_get(NULL, instance, "write", 5, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD ||
        method.bits != KILN_C99_METHOD_NODE_RESPONSE_WRITE) return 7;
    return 0;
}
"#;
        let run = run_with_main(
            &generated,
            wrapper,
            "native-server-response-prototype-graph",
        );
        assert!(
            run.status.success(),
            "native ServerResponse prototype graph failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_intrinsic_prototype_tables_can_be_relinked() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("empty program emits runtime prelude");
        let wrapper = r#"
int main(void) {
    kiln_value intrinsic = kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
    kiln_value parent;
    kiln_value observed;
    kiln_value result;
    kiln_value arguments[2];
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_c99_new_object(&parent) != KILN_C99_OK) return 1;
    arguments[0] = intrinsic;
    arguments[1] = parent;
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF,
                               kiln_c99_undefined(), arguments, 2, 0, &thrown) != KILN_C99_OK)
        return 2;
    if (result.tag != KILN_VALUE_BUILTIN || result.bits != intrinsic.bits) return 3;
    if (kiln_c99_invoke_method(NULL, &observed, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                               kiln_c99_undefined(), &intrinsic, 1, 0, &thrown) != KILN_C99_OK)
        return 4;
    if (observed.tag != KILN_VALUE_OBJECT || observed.bits != parent.bits) return 5;
    return 0;
}
"#;
        let run = run_with_main(&generated, wrapper, "intrinsic-prototype-relink");
        assert!(
            run.status.success(),
            "intrinsic prototype relink failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_promise_constructor_exposes_a_distinct_prototype_table() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("empty program emits runtime prelude");
        let wrapper = r#"
int main(void) {
    kiln_value constructor = kiln_c99_builtin(KILN_C99_BUILTIN_PROMISE);
    kiln_value prototype;
    kiln_value method;
    kiln_value owner;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_c99_get(NULL, constructor, "prototype", 9, &prototype, 0, &thrown) != KILN_C99_OK)
        return 1;
    if (prototype.tag != KILN_VALUE_BUILTIN ||
        prototype.bits != KILN_C99_BUILTIN_PROMISE_PROTOTYPE) return 2;
    if (prototype.bits == constructor.bits) return 3;
    if (kiln_c99_get(NULL, prototype, "then", 4, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD || method.bits != KILN_C99_METHOD_THEN) return 4;
    if (kiln_c99_get(NULL, prototype, "constructor", 11, &owner, 0, &thrown) != KILN_C99_OK ||
        owner.tag != KILN_VALUE_BUILTIN || owner.bits != KILN_C99_BUILTIN_PROMISE) return 5;
    return 0;
}
"#;
        let run = run_with_main(&generated, wrapper, "promise-prototype-table");
        assert!(
            run.status.success(),
            "Promise prototype table failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_numeric_operand_order_matches_symbol_conversion_rules() {
        for (operator, expected_value_of_calls) in [
            (BinaryOp::Multiply, 0),
            (BinaryOp::Add, 1),
            (BinaryOp::LessThan, 1),
        ] {
            let generated = emit(&Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: vec![
                    Constant::String("valueOf".to_owned()),
                    Constant::String("called".to_owned()),
                    Constant::Number(1.0f64.to_bits()),
                ],
                code: vec![
                    Instruction::LoadBuiltin(Builtin::Symbol),
                    Instruction::Call { argc: 0 },
                    Instruction::MakeFunction(0),
                    Instruction::Call { argc: 0 },
                    Instruction::Binary(operator),
                    Instruction::Return,
                ],
                local_count: 0,
                functions: vec![
                    FunctionProto {
                        span: kiln_ir::Span::default(),
                        prologue_end: 0,
                        asynchronous: false,
                        name: Some("right".to_owned()),
                        arrow: false,
                        generator: false,
                        async_generator: false,
                        parameter_count: 0,
                        declared_length: 0,
                        mapped_arguments: false,
                        local_count: 0,
                        code: vec![
                            Instruction::Const(0),
                            Instruction::MakeFunction(1),
                            Instruction::NewObject { count: 1 },
                            Instruction::Return,
                        ],
                        upvalues: Vec::new(),
                    },
                    FunctionProto {
                        span: kiln_ir::Span::default(),
                        prologue_end: 0,
                        asynchronous: false,
                        name: Some("valueOf".to_owned()),
                        arrow: false,
                        generator: false,
                        async_generator: false,
                        parameter_count: 0,
                        declared_length: 0,
                        mapped_arguments: false,
                        local_count: 0,
                        code: vec![
                            Instruction::Const(1),
                            Instruction::HostCall {
                                op: HostOp::WriteStdout,
                                argc: 1,
                            },
                            Instruction::Const(2),
                            Instruction::Return,
                        ],
                        upvalues: Vec::new(),
                    },
                ],
            })
            .expect("symbol conversion order probe emits");
            let wrapper = format!(
                r#"
static int value_of_calls;
static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != UINT16_C(1) || argc != 1 || arguments == NULL || result == NULL ||
        arguments[0].tag != KILN_VALUE_STRING || arguments[0].len != 6 ||
        memcmp(arguments[0].ptr, "called", 6) != 0) return 90;
    ++value_of_calls;
    *result = kiln_c99_undefined();
    return KILN_C99_OK;
}}
int main(void) {{
    kiln_host host = {{host_call, NULL}};
    kiln_value result = kiln_c99_undefined();
    int status = {symbol}(&host, &result);
    if (value_of_calls != {expected_value_of_calls}) return 10;
    return status == KILN_C99_THROWN ? 0 : 11;
}}
"#,
                symbol = generated.symbol,
                expected_value_of_calls = expected_value_of_calls
            );
            let run = run_with_main(&generated, &wrapper, "symbol-operand-order");
            assert!(
                run.status.success(),
                "{operator:?} operand order failed ({:?}): {}",
                run.status.code(),
                String::from_utf8_lossy(&run.stderr)
            );
        }
    }

    #[test]
    fn executable_symbol_registry_is_shared_and_distinct_from_unique_symbols() {
        let generated = emit(&program(
            vec![Constant::String("fastify-error-generic".into())],
            vec![Instruction::Return],
            0,
        ))
        .expect("symbol registry probe emits");
        let wrapper = r#"
int main(void) {
    uint64_t realm;
    kiln_value key = kiln_c99_copy_text("fastify-error-generic", 21);
    kiln_value first;
    kiln_value second;
    kiln_value unique;
    kiln_value recovered;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_symbol_for(&first, key) != KILN_C99_OK) return 1;
    if (kiln_c99_symbol_for(&second, key) != KILN_C99_OK) return 2;
    if (!kiln_c99_strict_equal(first, second)) return 3;
    recovered = kiln_c99_symbol_key_for(first);
    if (recovered.tag != KILN_VALUE_STRING || recovered.len != key.len ||
        memcmp(recovered.ptr, key.ptr, key.len) != 0) return 4;
    if (kiln_c99_new_symbol(&unique, key) != KILN_C99_OK) return 5;
    if (kiln_c99_strict_equal(first, unique)) return 6;
    if (kiln_c99_symbol_key_for(unique).tag != KILN_VALUE_UNDEFINED) return 7;
    if (kiln_c99_property_key(first).len == key.len &&
        memcmp(kiln_c99_property_key(first).ptr, key.ptr, key.len) == 0) return 8;
    if (kiln_c99_text(first).len != 29 ||
        memcmp(kiln_c99_text(first).ptr, "Symbol(fastify-error-generic)", 29) != 0) return 9;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 10;
}
"#
        .to_string();
        let run = run_with_main(&generated, &wrapper, "symbol-registry");
        assert!(
            run.status.success(),
            "native symbol registry failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_reflective_object_statics_dispatch_without_a_receiver() {
        let generated = emit(&program(
            vec![Constant::Undefined],
            vec![Instruction::Const(0), Instruction::Return],
            0,
        ))
        .expect("reflective static probe emits");
        let wrapper = r#"
int main(void) {
    uint64_t realm;
    kiln_value target;
    kiln_value descriptor;
    kiln_value key = kiln_c99_copy_text("answer", 6);
    kiln_value value = kiln_c99_number(42.0);
    kiln_value args[3];
    kiln_value result;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_new_object(&target) != KILN_C99_OK ||
        kiln_c99_new_object(&descriptor) != KILN_C99_OK) return 2;
    if (kiln_c99_put(NULL, descriptor, "value", 5, value, 0, NULL) != KILN_C99_OK) return 3;
    args[0] = target;
    args[1] = key;
    args[2] = descriptor;
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY,
                               kiln_c99_undefined(), args, 3, 0, &thrown) != KILN_C99_OK) return 4;
    if (!kiln_c99_strict_equal(result, target)) return 5;
    if (kiln_c99_get(NULL, target, "answer", 6, &result, 0, &thrown) != KILN_C99_OK) return 6;
    if (result.tag != KILN_VALUE_NUMBER || kiln_c99_as_number(result) != 42.0) return 7;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 8;
}
"#;
        let run = run_with_main(&generated, wrapper, "object-define-property-static");
        assert!(
            run.status.success(),
            "Object.defineProperty static dispatch failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn logical_path_diagnostics_name_source_and_operation() {
        let module = ModuleProgram {
            program: program(
                vec![Constant::String("defineProperty".into())],
                vec![
                    Instruction::LoadBuiltin(kiln_ir::Builtin::Object),
                    Instruction::CallMethod { name: 0, argc: 0 },
                    Instruction::Return,
                ],
                0,
            ),
            dependencies: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            strictness: kiln_ir::Strictness::default(),
        };
        let generated =
            emit_module_with_logical_path(&module, "node_modules/@fastify/error/index.js")
                .expect("logical module emits");
        assert!(
            generated
                .source
                .contains(
                    "kiln_c99_note_refusal(\"node_modules/@fastify/error/index.js\", (size_t)1, \"CallMethod(defineProperty)\", (size_t)kiln_c99_diagnostic_active_depth)"
                )
        );
        assert!(
            !generated
                .source
                .contains("kiln_c99_diagnostic_site = kiln_c99_diagnostic_site"),
            "diagnostic reset must not be substituted into the site expression"
        );
        assert!(
            generated
                .source
                .contains("kiln_c99_diagnostic_detail = NULL;")
        );
        assert!(
            generated
                .source
                .contains("Object.defineProperty invalid key or arity")
        );
    }

    /// Every value kind that answers "yes" to `kiln_c99_is_callable` converts to text.
    ///
    /// `Function.prototype.toString` reaches a value by two routes -- `String(f)` through the
    /// text conversion and `f.toString()` through a member lookup -- and both end at
    /// `kiln_c99_text`. `kiln_c99_is_callable` listed six tags and `kiln_c99_text` handled three,
    /// so a bound function, a promise settler and a `require` all fell to `default:` and answered
    /// `undefined`. Nothing failed: `String(f)` was right for a plain function, and no probe
    /// called the method on any of the other three.
    ///
    /// This is the pairing, not the symptom. Adding a callable tag without a text case is now a
    /// test failure here rather than a wrong answer discovered later by whoever happens to call
    /// `toString` on the one shape nobody wrote a probe for.
    #[test]
    fn callable_text_covers_every_callable_tag() {
        let runtime = HEADER;

        fn body<'a>(runtime: &'a str, signature: &str) -> &'a str {
            let start = runtime
                .split(signature)
                .nth(1)
                .unwrap_or_else(|| panic!("{signature} must exist"));
            // The C function ends at the first line that closes it in column zero.
            let end = start.find("\n}").expect("function must be closed");
            &start[..end]
        }

        let callable = body(
            runtime,
            "static int kiln_c99_is_callable(kiln_value value) {",
        );
        let text = body(
            runtime,
            "static kiln_value kiln_c99_text(kiln_value value) {",
        );

        let tags: Vec<&str> = callable
            .split("value.tag == ")
            .skip(1)
            .filter_map(|rest| {
                rest.split(|c: char| !c.is_ascii_uppercase() && c != '_')
                    .next()
            })
            .filter(|tag| tag.starts_with("KILN_VALUE_"))
            .collect();
        // A guard that has quietly stopped reading the thing it checks is worse than none.
        assert!(
            tags.len() >= 5,
            "read only {tags:?} out of `kiln_c99_is_callable`; the parse has stopped working"
        );
        assert!(
            text.contains("case KILN_VALUE_FUNCTION:"),
            "the parse of `kiln_c99_text` has stopped working"
        );

        let missing: Vec<&&str> = tags
            .iter()
            .filter(|tag| !text.contains(&format!("case {tag}:")))
            .collect();
        assert!(
            missing.is_empty(),
            "`kiln_c99_is_callable` accepts {missing:?}, which `kiln_c99_text` has no case for. \
             Each would fall to `default:` and answer `undefined`, so `String(x)` and \
             `x.toString()` on that shape would report a value the program never wrote. A \
             callable with no source text of its own takes the NativeFunction form, which is what \
             the interpreter answers and what the specification allows for a built-in."
        );
    }

    /// Every tag `typeof` calls an object also converts, is truthy, and names itself.
    ///
    /// The sibling of `callable_text_covers_every_callable_tag`, and it exists because the same
    /// mistake had been made four more times in four more places. `KILN_VALUE_TEMPORAL` was
    /// absent from `kiln_c99_text`, from `kiln_c99_truthy`, from the `Object.prototype.toString`
    /// tag switch and from `kiln_c99_is_reference`, so a `Temporal` value converted to
    /// `undefined`, was *falsy*, named itself `[object Object]`, and was never reduced by a
    /// coercing operator at all -- `"x" + aPlainDate` appended nothing and the program carried
    /// on. Promises, generators and collections were missing from the text conversion too.
    ///
    /// Each of those is a classifier written as a list of the kinds it knows about, and a list
    /// goes stale silently. `kiln_c99_typeof` is the one that cannot: a value the program can
    /// hold has to answer *something* there, so its "object" arm is the closed enumeration of
    /// the exotic object kinds and the right thing to check the others against.
    ///
    /// This does not check the answers -- `conformance/native/exotic-conversions.ts` drives every
    /// kind through every implicit conversion against the interpreter for that. It checks that
    /// the arm exists, which is what stops the next tag from being wrong in four places at once.
    #[test]
    fn every_object_tag_converts_and_is_truthy_and_names_itself() {
        let runtime = HEADER;

        fn body<'a>(runtime: &'a str, signature: &str) -> &'a str {
            let start = runtime
                .split(signature)
                .nth(1)
                .unwrap_or_else(|| panic!("{signature} must exist"));
            let end = start.find("\n}").expect("function must be closed");
            &start[..end]
        }

        let typeof_body = body(
            runtime,
            "static kiln_value kiln_c99_typeof(kiln_value value) {",
        );
        // The run of `case` labels that falls through to the "object" answer.
        let object_arm = typeof_body
            .split("case KILN_VALUE_OBJECT:")
            .nth(1)
            .expect("`kiln_c99_typeof` must answer \"object\" for something");
        let object_arm = &object_arm[..object_arm
            .find("object_text")
            .expect("the \"object\" arm must end at its answer")];
        let mut tags: Vec<&str> = vec!["KILN_VALUE_OBJECT"];
        tags.extend(
            object_arm
                .split("case ")
                .skip(1)
                .filter_map(|rest| rest.split(':').next())
                .filter(|tag| tag.starts_with("KILN_VALUE_")),
        );
        // A guard that has quietly stopped reading the thing it checks is worse than none.
        assert!(
            tags.len() >= 8,
            "read only {tags:?} out of `kiln_c99_typeof`; the parse has stopped working"
        );
        assert!(
            tags.contains(&"KILN_VALUE_TEMPORAL"),
            "the parse of `kiln_c99_typeof` has stopped working"
        );

        // An array and a plain object are deliberately outside `kiln_c99_has_intrinsic_text`:
        // their text depends on what they hold, so only the `valueOf`/`toString` walk can ask.
        let intrinsic_text = body(
            runtime,
            "static int kiln_c99_has_intrinsic_text(kiln_value value) {",
        );
        for (what, region, exempt) in [
            (
                "kiln_c99_text",
                body(
                    runtime,
                    "static kiln_value kiln_c99_text(kiln_value value) {",
                ),
                &[][..],
            ),
            (
                "kiln_c99_truthy",
                body(runtime, "static int kiln_c99_truthy(kiln_value value) {"),
                &[][..],
            ),
            (
                "kiln_c99_has_intrinsic_text",
                intrinsic_text,
                &["KILN_VALUE_OBJECT", "KILN_VALUE_ARRAY"][..],
            ),
        ] {
            let missing: Vec<&&str> = tags
                .iter()
                .filter(|tag| !exempt.contains(tag))
                .filter(|tag| !region.contains(&format!("case {tag}:")))
                .collect();
            assert!(
                missing.is_empty(),
                "`kiln_c99_typeof` answers \"object\" for {missing:?}, which `{what}` has no \
                 case for. Each would fall to its `default:`, and every one of those defaults is \
                 a wrong answer rather than a refusal: `undefined` from the text conversion, \
                 *falsy* from the truthiness test. A `Temporal` value was in exactly that state \
                 and no probe converted one."
            );
        }
    }

    #[test]
    #[ignore = "ADR 0036 migration gate: value-kernel state still ships in every module"]
    fn emitted_modules_do_not_own_realm_value_kernel_state() {
        let generated =
            emit(&program(vec![], vec![Instruction::Return], 0)).expect("empty program emits");
        for declaration in [
            "static kiln_value kiln_c99_globals;",
            "static kiln_value kiln_c99_process;",
            "static kiln_value kiln_c99_atomics;",
            "static kiln_value *kiln_c99_symbols;",
            "static kiln_method_store *kiln_c99_fixture_method_stores;",
            "static kiln_native_constructor kiln_c99_native_constructors[]",
        ] {
            assert!(
                !generated.source.contains(declaration),
                "realm state remains private to one generated translation unit: {declaration}"
            );
        }
    }

    #[test]
    fn every_mutable_c99_static_has_an_explicit_owner() {
        let generated =
            emit(&program(vec![], vec![Instruction::Return], 0)).expect("empty program emits");
        let mut discovered = generated
            .source
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let declaration = line.split('=').next().unwrap_or(line);
                let name_start = line.find("kiln_c99_")?;
                let before_name = &line[..name_start];
                let immutable = if before_name.contains('*') {
                    before_name.trim_end().ends_with("* const")
                } else {
                    before_name.starts_with("static const ")
                };
                if !line.starts_with("static ")
                    || immutable
                    || declaration.contains('(')
                    || !line.contains("kiln_c99_")
                    || !line.ends_with(';')
                {
                    return None;
                }
                let tail = &line[name_start..];
                let end = tail
                    .find(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                    .unwrap_or(tail.len());
                Some(tail[..end].to_owned())
            })
            .collect::<Vec<_>>();
        discovered.sort();
        discovered.dedup();
        let mut classified = C99_MUTABLE_STATE_OWNERS
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect::<Vec<_>>();
        classified.sort();
        assert_eq!(
            discovered, classified,
            "mutable generated-C state must say whether the realm, dynamic frame, or test fixture owns it"
        );
        assert!(
            !generated
                .source
                .contains("static kiln_value kiln_c99_globals;"),
            "globalThis identity must live in the active realm slot, not one module"
        );
        assert!(
            generated.source.contains("kiln_realm_value_slot("),
            "generated value operations must reach realm singleton identity through the typed slot ABI"
        );
    }

    #[test]
    fn structural_instruction_refusals_always_name_their_owner() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("structural refusal probe emits");
        assert!(
            generated.source.contains("kiln_c99_note_refusal"),
            "runtime must include the refusal provenance boundary"
        );
        let emitter = include_str!("lib.rs");
        let instruction_emitter = emitter
            .split("fn emit_instruction(")
            .nth(1)
            .expect("instruction emitter")
            .split("fn instruction_diagnostic(")
            .next()
            .expect("instruction emitter end");
        let templates = instruction_emitter
            .split("let refusal_target =")
            .next()
            .expect("instruction templates");
        assert!(
            !templates
                .lines()
                .skip(35)
                .any(|line| line.contains("return KILN_C99_CONTROL_ERROR;")),
            "instruction templates must route structural refusals through direct_refusal"
        );
    }

    #[test]
    // `f16round` rather than an invented name: an absent name on a table-driven built-in is
    // `undefined` now, exactly as the language says, so a made-up one no longer refuses and this
    // probe would have been testing nothing. `Math.f16round` is a real member this backend has
    // not built, so it is in the backlog and still refuses by name -- which is the provenance
    // this test is about.
    fn nested_call_refusal_preserves_the_failing_instruction_without_blaming_the_callee() {
        let generated = emit(&Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::String("f16round".into())],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: vec![FunctionProto {
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: Some("read_unimplemented_math_property".into()),
                arrow: false,
                generator: false,
                async_generator: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 0,
                code: vec![
                    Instruction::LoadBuiltin(Builtin::Math),
                    Instruction::GetProperty(0),
                    Instruction::Return,
                ],
                upvalues: Vec::new(),
            }],
        })
        .expect("nested refusal probe emits");
        let wrapper = format!(
            r#"
int main(void) {{
    kiln_value result = kiln_c99_undefined();
    int status = {symbol}(NULL, &result);
    if (status != KILN_C99_CONTROL_ERROR) return 1;
    if (kiln_c99_diagnostic_site == NULL || kiln_c99_diagnostic_operation == NULL) return 2;
    if (kiln_c99_diagnostic_instruction != (size_t)1) return 3;
    if (strcmp(kiln_c99_diagnostic_operation, "GetProperty(f16round)") != 0) return 4;
    if (kiln_c99_diagnostic_detail == NULL ||
        strcmp(kiln_c99_diagnostic_detail, "GetProperty lookup refused") != 0) return 5;
    if (kiln_c99_diagnostic_receiver_tag != KILN_VALUE_BUILTIN) return 6;
    return 0;
}}
"#,
            symbol = generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "nested-call-refusal-provenance");
        assert!(
            run.status.success(),
            "nested refusal provenance failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn unowned_function_refusal_names_its_generated_source_and_function() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("diagnostic source probe emits");
        let wrapper = r#"
static int refuses_before_instruction(kiln_host *host, kiln_value *result,
                                      kiln_value *upvalues, kiln_value receiver,
                                      const kiln_value *args, size_t argc, int depth,
                                      kiln_value *thrown, kiln_generator *frame) {
    (void)host; (void)result; (void)upvalues; (void)receiver; (void)args;
    (void)argc; (void)depth; (void)thrown; (void)frame;
    return KILN_C99_CONTROL_ERROR;
}
int main(void) {
    uint64_t realm;
    kiln_value function;
    kiln_value result = kiln_c99_undefined();
    kiln_value thrown = kiln_c99_undefined();
    kiln_value diagnostic;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_make_closure(&function, refuses_before_instruction, "initialize", 10,
                              NULL, 0, "node_modules/example/index.js", 0, 0, 0, 0, 0, 0,
                              1, kiln_c99_undefined(), kiln_c99_undefined(), 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK) return 1;
    if (kiln_c99_call(NULL, &result, function, kiln_c99_undefined(), NULL, 0, 0,
                      &thrown) != KILN_C99_CONTROL_ERROR) return 2;
    diagnostic = kiln_c99_control_diagnostic();
    if (diagnostic.tag != KILN_VALUE_STRING) return 3;
    if (strstr(diagnostic.ptr, "module=node_modules/example/index.js") == NULL) return 4;
    if (strstr(diagnostic.ptr, "operation=FunctionBody") == NULL) return 5;
    if (strstr(diagnostic.ptr, "function=initialize") == NULL) return 6;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 7;
}
"#;
        let run = run_with_main(&generated, wrapper, "function-refusal-source");
        assert!(
            run.status.success(),
            "function refusal source failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn an_instruction_replaces_the_function_body_fallback_at_the_same_depth() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("diagnostic ownership probe emits");
        let wrapper = r#"
int main(void) {
    kiln_c99_diagnostic_site = "callee.js";
    kiln_c99_diagnostic_instruction = 0;
    kiln_c99_diagnostic_operation = "FunctionBody";
    kiln_c99_diagnostic_depth = 2;
    kiln_c99_diagnostic_function = "callee";
    kiln_c99_diagnostic_function_len = 6;
    kiln_c99_diagnostic_receiver_tag = KILN_VALUE_FUNCTION;
    kiln_c99_diagnostic_detail =
        "generated function body refused before an instruction owned the failure";
    kiln_c99_note_refusal("callee.js", 37, "CallMethod(listen)", 2);
    if (kiln_c99_diagnostic_instruction != 37) return 1;
    if (strcmp(kiln_c99_diagnostic_operation, "CallMethod(listen)") != 0) return 2;
    if (kiln_c99_diagnostic_function != NULL) return 3;
    if (kiln_c99_diagnostic_detail != NULL) return 4;
    if (kiln_c99_diagnostic_receiver_tag != 0) return 5;
    return 0;
}
"#;
        let run = run_with_main(&generated, wrapper, "instruction-replaces-body-fallback");
        assert!(
            run.status.success(),
            "diagnostic ownership failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    fn program(constants: Vec<Constant>, code: Vec<Instruction>, local_count: u32) -> Program {
        Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants,
            code,
            local_count,
            functions: Vec::new(),
        }
    }

    fn module(
        program: Program,
        imports: Vec<kiln_ir::ImportBinding>,
        exports: Vec<kiln_ir::ExportBinding>,
    ) -> ModuleProgram {
        ModuleProgram {
            program,
            dependencies: Vec::new(),
            imports,
            exports,
            strictness: kiln_ir::Strictness::default(),
        }
    }

    /// The engine half of the provider ABI, for tests that link the vendored C number runtime.
    ///
    /// `kiln-native-number`'s C is a *test fixture*: production links the Rust provider archive,
    /// which defines these. Tests here exercise the emitted C's own logic and none of them
    /// compiles a pattern or does arbitrary-precision arithmetic -- so a fixture that refuses
    /// every one is honest, and a test that did need one would get a visible error rather than a
    /// quiet wrong answer. Such a test belongs in `conformance/native`, where the real archive is
    /// linked.
    const REGEXP_FIXTURE: &str = r#"
int32_t kiln_regexp_compile(const unsigned char *pattern, size_t pattern_len,
                            const unsigned char *flags, size_t flags_len, void **out_handle) {
    (void)pattern; (void)pattern_len; (void)flags; (void)flags_len; (void)out_handle;
    return -1;
}
void kiln_regexp_free(void *handle) { (void)handle; }
int32_t kiln_regexp_search(void *handle, const uint16_t *subject, size_t subject_len, size_t start,
                           int64_t *out_ranges, size_t out_capacity) {
    (void)handle; (void)subject; (void)subject_len; (void)start; (void)out_ranges;
    (void)out_capacity;
    return -1;
}
size_t kiln_regexp_group_count(const void *handle) { (void)handle; return 0; }
int32_t kiln_regexp_group_name(const void *handle, size_t index, unsigned char *buffer,
                               size_t capacity) {
    (void)handle; (void)index; (void)buffer; (void)capacity;
    return -1;
}
int32_t kiln_bigint_parse(const unsigned char *text, size_t len, uint32_t radix, void **out) {
    (void)text; (void)len; (void)radix; (void)out;
    return -1;
}
int32_t kiln_bigint_from_double(double value, void **out) { (void)value; (void)out; return -1; }
void kiln_bigint_free(void *handle) { (void)handle; }
double kiln_bigint_to_double(const void *handle) { (void)handle; return 0.0; }
int32_t kiln_bigint_to_string(const void *handle, uint32_t radix, unsigned char *buffer,
                              size_t capacity) {
    (void)handle; (void)radix; (void)buffer; (void)capacity;
    return -1;
}
int32_t kiln_bigint_binary(uint32_t operation, const void *left, const void *right, void **out) {
    (void)operation; (void)left; (void)right; (void)out;
    return -1;
}
int32_t kiln_bigint_negate(const void *handle, void **out) { (void)handle; (void)out; return -1; }
int32_t kiln_bigint_compare(const void *left, const void *right) {
    (void)left; (void)right;
    return 0;
}
int32_t kiln_bigint_is_zero(const void *handle) { (void)handle; return 1; }
int32_t kiln_bigint_to_i128(const void *handle, int64_t *out_high, uint64_t *out_low) {
    (void)handle; (void)out_high; (void)out_low;
    return -1;
}
int32_t kiln_temporal_from_fields(uint32_t kind, const double *fields, size_t field_count,
                                  const unsigned char *text, const int64_t *text_lengths,
                                  size_t text_count, const unsigned char *option_text,
                                  const int64_t *option_text_lengths, size_t option_text_count,
                                  void **out_handle) {
    (void)kind; (void)fields; (void)field_count; (void)text; (void)text_lengths;
    (void)text_count; (void)option_text; (void)option_text_lengths; (void)option_text_count;
    (void)out_handle;
    return -1;
}
int32_t kiln_temporal_duration_from_fields(const double *fields, size_t field_count,
                                           void **out_handle) {
    (void)fields; (void)field_count; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_construct(uint32_t kind, const double *fields, size_t field_count,
                                const unsigned char *text, const int64_t *text_lengths,
                                size_t text_count, int64_t epoch_high, uint64_t epoch_low,
                                void **out_handle) {
    (void)kind; (void)fields; (void)field_count; (void)text; (void)text_lengths;
    (void)text_count; (void)epoch_high; (void)epoch_low; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_parse(uint32_t kind, const unsigned char *bytes, size_t len,
                            const unsigned char *option_text,
                            const int64_t *option_text_lengths, size_t option_text_count,
                            void **out_handle) {
    (void)kind; (void)bytes; (void)len; (void)option_text; (void)option_text_lengths;
    (void)option_text_count; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_now(uint32_t kind, int64_t high, uint64_t low, const unsigned char *zone,
                          size_t zone_len, void **out_handle) {
    (void)kind; (void)high; (void)low; (void)zone; (void)zone_len; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_instant_from_epoch_milliseconds(double milliseconds, void **out_handle) {
    (void)milliseconds; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_instant_from_epoch_nanoseconds(int64_t high, uint64_t low,
                                                     void **out_handle) {
    (void)high; (void)low; (void)out_handle;
    return -1;
}
void kiln_temporal_free(void *handle) { (void)handle; }
int32_t kiln_temporal_kind(const void *handle) { (void)handle; return -1; }
int32_t kiln_temporal_get(const void *handle, const unsigned char *name, size_t name_len,
                          double *out_number, unsigned char *buffer, size_t capacity,
                          int32_t *out_length) {
    (void)handle; (void)name; (void)name_len; (void)out_number; (void)buffer; (void)capacity;
    (void)out_length;
    return -1;
}
int32_t kiln_temporal_to_string(const void *handle, unsigned char *buffer, size_t capacity) {
    (void)handle; (void)buffer; (void)capacity;
    return -1;
}
int32_t kiln_temporal_compare(const void *left, const void *right, const void *relative_to,
                              const unsigned char *option_text,
                              const int64_t *option_text_lengths, size_t option_text_count,
                              int32_t *out_order) {
    (void)left; (void)right; (void)relative_to; (void)option_text; (void)option_text_lengths;
    (void)option_text_count; (void)out_order;
    return -1;
}
int32_t kiln_temporal_to_kind(const void *handle, uint32_t kind, void **out_handle) {
    (void)handle; (void)kind; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_last_error(int32_t *out_kind, unsigned char *buffer, size_t capacity) {
    (void)out_kind; (void)buffer; (void)capacity;
    return -1;
}
"#;

    /// Link-only Realm provider for backend unit tests.
    ///
    /// Scheduler behavior is covered through the real Rust archive in `kiln-native`; these tests
    /// exercise generated scalar C directly. The fixture therefore owns and disposes any work it
    /// is unexpectedly given without duplicating a second event loop in the backend test module.
    const REALM_FIXTURE: &str = r#"
static uint64_t kiln_test_next_timer_id = 1;
static int kiln_test_realm_active = 0;
static uint64_t kiln_test_symbol_id = 1;
static char kiln_test_symbol_key[256];
static uint64_t kiln_test_symbol_key_len = 0;
static uint32_t kiln_test_callable_tags[256];
static uint64_t kiln_test_callable_codes[256];
static void *kiln_test_callable_stores[256];
static size_t kiln_test_callable_store_count = 0;
static kiln_value kiln_test_value_slots[5];
static uint64_t kiln_test_entropy_seed = 0;
static uint64_t kiln_test_entropy_state = 0;
static int kiln_test_entropy_initialized = 0;

int kiln_realm_create(uint64_t *out_realm) {
    if (out_realm == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_realm = 1;
    return KILN_C99_OK;
}
int kiln_realm_enter(uint64_t realm, kiln_realm_entry_fn entry, void *context) {
    int status;
    if (realm != 1 || entry == NULL || kiln_test_realm_active) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    kiln_test_realm_active = 1;
    status = entry(context);
    kiln_test_realm_active = 0;
    return status;
}
int kiln_realm_drive(uint64_t realm, void *host, void *thrown) {
    (void)host;
    (void)thrown;
    return realm == 1 ? KILN_C99_OK : KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_destroy(uint64_t realm) {
    if (realm != 1 || kiln_test_realm_active) return KILN_C99_INVALID_ARGUMENT;
    return KILN_C99_OK;
}
/* Here rather than beside the other provider stubs, because its argument is a struct the runtime
 * prelude declares and this fixture is the first text that follows the prelude. */
int32_t kiln_temporal_invoke(const kiln_temporal_call *call) { (void)call; return -1; }
int kiln_realm_is_active(void) { return kiln_test_realm_active; }
int kiln_realm_diagnostic_reset(void) { return KILN_C99_OK; }
int kiln_realm_diagnostic_note(const uint8_t *site, uint64_t site_len, uint64_t instruction,
                               const uint8_t *operation, uint64_t operation_len, uint64_t depth,
                               const uint8_t *detail, uint64_t detail_len,
                               const uint8_t *function, uint64_t function_len,
                               uint64_t method, uint64_t receiver_tag, int has_method) {
    (void)site; (void)site_len; (void)instruction; (void)operation; (void)operation_len;
    (void)depth; (void)detail; (void)detail_len; (void)function; (void)function_len;
    (void)method; (void)receiver_tag; (void)has_method;
    return KILN_C99_OK;
}
int kiln_realm_diagnostic_render(uint8_t *out, uint64_t capacity, uint64_t *out_len) {
    (void)out; (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
/* `node:path`'s decompositions come from the Rust provider, which this test host does not link:
 * the backend's own tests compile generated C against a shim, not against the shipped archive.
 * Refusing here is the honest answer -- a second implementation living in the test host is exactly
 * the drift the provider exists to end. `kiln test conformance/*` and `scripts/native.sh` run
 * against the real archive and are where these are exercised. */
int kiln_node_path_part(uint32_t part, const uint8_t *path, uint64_t path_len,
                        const uint8_t *suffix, uint64_t suffix_len, uint8_t *out,
                        uint64_t capacity, uint64_t *out_len) {
    (void)part; (void)path; (void)path_len; (void)suffix; (void)suffix_len; (void)out;
    (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_node_path_relative(const uint8_t *from, uint64_t from_len, const uint8_t *to,
                            uint64_t to_len, uint8_t *out, uint64_t capacity, uint64_t *out_len) {
    (void)from; (void)from_len; (void)to; (void)to_len; (void)out; (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_symbol_for(const uint8_t *key, uint64_t key_len, uint64_t *out_id) {
    if (!kiln_test_realm_active || out_id == NULL || key_len > sizeof kiln_test_symbol_key) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    if (kiln_test_symbol_key_len == 0) {
        memcpy(kiln_test_symbol_key, key, (size_t)key_len);
        kiln_test_symbol_key_len = key_len;
    } else if (key_len != kiln_test_symbol_key_len ||
               memcmp(kiln_test_symbol_key, key, (size_t)key_len) != 0) {
        ++kiln_test_symbol_id;
        memcpy(kiln_test_symbol_key, key, (size_t)key_len);
        kiln_test_symbol_key_len = key_len;
    }
    *out_id = kiln_test_symbol_id;
    return KILN_C99_OK;
}
int kiln_realm_symbol_key_for(uint64_t id, const uint8_t **out_key, uint64_t *out_key_len) {
    if (!kiln_test_realm_active || id != kiln_test_symbol_id || out_key == NULL ||
        out_key_len == NULL) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    *out_key = (const uint8_t *)kiln_test_symbol_key;
    *out_key_len = kiln_test_symbol_key_len;
    return KILN_C99_OK;
}
int kiln_realm_callable_store(uint32_t tag, uint64_t code, void *candidate,
                              kiln_drop_opaque_fn drop_candidate, void **out_store) {
    size_t index;
    if (!kiln_test_realm_active || out_store == NULL ||
        ((candidate == NULL) != (drop_candidate == NULL))) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    if (candidate == NULL) {
        *out_store = NULL;
        return KILN_C99_OK;
    }
    for (index = 0; index < kiln_test_callable_store_count; ++index) {
        if (kiln_test_callable_tags[index] == tag && kiln_test_callable_codes[index] == code) {
            *out_store = kiln_test_callable_stores[index];
            return KILN_C99_OK;
        }
    }
    if (kiln_test_callable_store_count == 256) return KILN_C99_OUT_OF_MEMORY;
    index = kiln_test_callable_store_count++;
    kiln_test_callable_tags[index] = tag;
    kiln_test_callable_codes[index] = code;
    kiln_test_callable_stores[index] = candidate;
    *out_store = candidate;
    return KILN_C99_OK;
}
int kiln_realm_value_slot(uint32_t key, void **out_slot) {
    if (!kiln_test_realm_active || key == 0 || key > 5 || out_slot == NULL)
        return KILN_C99_INVALID_ARGUMENT;
    *out_slot = &kiln_test_value_slots[key - 1];
    return KILN_C99_OK;
}
int kiln_realm_entropy_word(uint64_t seed, uint64_t *out_word) {
    uint64_t mixed;
    if (!kiln_test_realm_active || out_word == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (!kiln_test_entropy_initialized) {
        kiln_test_entropy_seed = seed;
        kiln_test_entropy_state = seed;
        kiln_test_entropy_initialized = 1;
    } else if (kiln_test_entropy_seed != seed) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    kiln_test_entropy_state += UINT64_C(0x9E3779B97F4A7C15);
    mixed = kiln_test_entropy_state;
    mixed = (mixed ^ (mixed >> 30)) * UINT64_C(0xBF58476D1CE4E5B9);
    mixed = (mixed ^ (mixed >> 27)) * UINT64_C(0x94D049BB133111EB);
    *out_word = mixed ^ (mixed >> 31);
    return KILN_C99_OK;
}
void *kiln_realm_gc_current_heap(void) { return NULL; }
int kiln_realm_gc_attach(const kiln_gc_hooks *hooks, void *candidate, void **out_heap) {
    /* The fixture has no realm heap: every unit collects nothing and keeps its fallback heap. */
    (void)hooks;
    (void)candidate;
    if (out_heap != NULL) *out_heap = NULL;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_gc_register_roots(kiln_gc_roots_fn roots, void *context, uint32_t flags) {
    (void)roots;
    (void)context;
    (void)flags;
    return KILN_C99_OK;
}
int kiln_realm_gc_mark_roots(void *heap) {
    (void)heap;
    return KILN_C99_OK;
}
int kiln_realm_now(double *out_milliseconds) {
    if (!kiln_test_realm_active || out_milliseconds == NULL) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    *out_milliseconds = 0.0;
    return KILN_C99_OK;
}
int kiln_realm_now_nanoseconds(int64_t *out_high, uint64_t *out_low) {
    if (!kiln_test_realm_active || out_high == NULL || out_low == NULL) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    *out_high = 0;
    *out_low = 0;
    return KILN_C99_OK;
}
int kiln_realm_queue_microtask(kiln_realm_work_fn run,
                               kiln_realm_drop_fn drop,
                               void *work) {
    (void)run;
    if (drop != NULL) drop(work);
    return KILN_C99_OK;
}
int kiln_realm_queue_next_tick(kiln_realm_work_fn run,
                               kiln_realm_drop_fn drop,
                               void *work) {
    (void)run;
    if (drop != NULL) drop(work);
    return KILN_C99_OK;
}
/* No launcher ran, so there is no command line. The prelude reads zero here and leaves
 * `process.argv` empty, which is what a harness-hosted realm should report. */
int kiln_realm_argv_count(uint64_t *out_count) {
    if (!kiln_test_realm_active || out_count == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_count = 0;
    return KILN_C99_OK;
}
int kiln_realm_argv_at(uint64_t index, const uint8_t **out_bytes, uint64_t *out_len) {
    (void)index;
    (void)out_bytes;
    (void)out_len;
    return KILN_C99_INVALID_ARGUMENT;
}
static int kiln_test_set_timer(double delay_ms,
                               kiln_realm_work_fn run,
                               kiln_realm_drop_fn drop,
                               void *work,
                               uint64_t *out_id) {
    (void)delay_ms;
    (void)run;
    if (out_id == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_id = kiln_test_next_timer_id++;
    if (drop != NULL) drop(work);
    return KILN_C99_OK;
}
int kiln_realm_set_timeout(double delay_ms,
                           kiln_realm_work_fn run,
                           kiln_realm_drop_fn drop,
                           void *work,
                           uint64_t *out_id) {
    return kiln_test_set_timer(delay_ms, run, drop, work, out_id);
}
int kiln_realm_set_interval(double delay_ms,
                            kiln_realm_work_fn run,
                            kiln_realm_drop_fn drop,
                            void *work,
                            uint64_t *out_id) {
    return kiln_test_set_timer(delay_ms, run, drop, work, out_id);
}
int kiln_realm_clear_timer(uint64_t id) {
    (void)id;
    return KILN_C99_OK;
}
int kiln_realm_start_active_host_op(const kiln_async_host_op *operation,
                                    kiln_realm_completion_fn run,
                                    kiln_realm_drop_fn drop,
                                    void *work,
                                    uint64_t *out_request) {
    (void)operation;
    (void)run;
    if (out_request != NULL) *out_request = 1;
    if (drop != NULL) drop(work);
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_cancel_active_request(uint64_t request) {
    (void)request;
    return KILN_REALM_ASYNC_NOT_FOUND;
}

/* Scalar backend fixtures never exercise Node HTTP; production artifacts link these symbols
 * from the Rust realm provider. */
int kiln_realm_http_server_attach_active(uint64_t resource, kiln_realm_http_event_fn run,
                                         kiln_realm_drop_fn drop, void *work) {
    (void)resource; (void)run; if (drop != NULL) drop(work); return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_server_attach_with_options_active(
    uint64_t resource, int allow_half_close, kiln_realm_http_event_fn run,
    kiln_realm_drop_fn drop, void *work) {
    (void)allow_half_close;
    return kiln_realm_http_server_attach_active(resource, run, drop, work);
}
int kiln_realm_http_server_close_active(uint64_t resource, int mode) {
    (void)resource; (void)mode;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_client_attach_active(uint64_t resource, kiln_realm_http_event_fn run,
                                         kiln_realm_drop_fn drop, void *work) {
    (void)resource; (void)run; if (drop != NULL) drop(work); return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_client_request_id_active(uint64_t *out_id) {
    if (out_id == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_id = 1;
    return KILN_C99_OK;
}
int kiln_realm_http_agent_acquire_active(const uint8_t *host, uint64_t host_len,
                                         uint16_t port, uint64_t request,
                                         uint64_t *out_resource) {
    (void)host; (void)host_len; (void)port; (void)request;
    if (out_resource == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_resource = 0;
    return KILN_REALM_HTTP_AGENT_CONNECT;
}
int kiln_realm_http_client_owner_active(uint64_t resource, void **out_owner) {
    (void)resource;
    if (out_owner == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_owner = NULL;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_agent_connected_active(const uint8_t *host, uint64_t host_len,
                                           uint16_t port, uint64_t resource,
                                           uint64_t request) {
    (void)host; (void)host_len; (void)port; (void)resource; (void)request;
    return 1;
}
int kiln_realm_http_agent_release_active(uint64_t resource, int reusable,
                                         uint64_t *out_request) {
    (void)resource; (void)reusable;
    if (out_request == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_request = 0;
    return KILN_REALM_HTTP_AGENT_IDLE;
}
int kiln_realm_http_client_start_active(uint64_t resource,
                                        uint64_t id,
                                        const kiln_realm_http_request *request,
                                        uint64_t high_water_mark) {
    (void)resource; (void)id; (void)request; (void)high_water_mark;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_client_write_active(uint64_t resource,
                                        uint64_t id,
                                        const uint8_t *data, uint64_t data_len) {
    (void)resource; (void)id; (void)data; (void)data_len; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_client_end_active(uint64_t resource,
                                      uint64_t id,
                                      const uint8_t *data, uint64_t data_len) {
    (void)resource; (void)id; (void)data; (void)data_len; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_response_start_active(uint64_t resource, uint64_t id,
                                          const kiln_realm_http_response *response,
                                          uint64_t high_water_mark) {
    (void)resource; (void)id; (void)response; (void)high_water_mark;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_response_write_active(uint64_t resource, uint64_t id,
                                          const uint8_t *data, uint64_t data_len) {
    (void)resource; (void)id; (void)data; (void)data_len; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_response_end_active(uint64_t resource, uint64_t id,
                                        const uint8_t *data, uint64_t data_len) {
    (void)resource; (void)id; (void)data; (void)data_len; return KILN_C99_INVALID_ARGUMENT;
}
"#;

    fn linked_source(module_source: &str, wrapper: &str) -> String {
        format!(
            "{}\n{}\n{}\n{}\n{}",
            kiln_native_number::runtime_source(),
            REGEXP_FIXTURE,
            module_source,
            REALM_FIXTURE,
            wrapper
        )
    }

    fn function(code: Vec<Instruction>) -> FunctionProto {
        FunctionProto {
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: None,
            arrow: false,
            generator: false,
            async_generator: false,
            parameter_count: 0,
            declared_length: 0,
            mapped_arguments: false,
            local_count: 0,
            code,
            upvalues: Vec::new(),
        }
    }

    #[test]
    fn program_hash_covers_function_table_and_proto_contents() {
        let base = program(Vec::new(), vec![Instruction::Return], 0);
        let with_function = |functions| Program {
            functions,
            ..base.clone()
        };
        let empty_hash = hash_program(&with_function(Vec::new()));
        let one_hash = hash_program(&with_function(vec![function(vec![Instruction::Return])]));
        assert_ne!(
            empty_hash, one_hash,
            "empty and nonempty tables must differ"
        );

        let mut changed_body = function(vec![Instruction::Return]);
        changed_body.code = vec![Instruction::Pop, Instruction::Return];
        assert_ne!(one_hash, hash_program(&with_function(vec![changed_body])));

        for change in [
            FunctionProto {
                prologue_end: 0,
                asynchronous: true,
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                prologue_end: 0,
                name: Some("named".into()),
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                prologue_end: 0,
                name: Some(String::new()),
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                prologue_end: 0,
                arrow: true,
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                prologue_end: 0,
                generator: true,
                async_generator: false,
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                prologue_end: 0,
                parameter_count: 1,
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                prologue_end: 0,
                local_count: 1,
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                prologue_end: 0,
                upvalues: vec![Upvalue::ParentLocal(0)],
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                prologue_end: 0,
                upvalues: vec![Upvalue::ParentUpvalue(0)],
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                prologue_end: 0,
                upvalues: vec![Upvalue::ParentLocal(1)],
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                prologue_end: 0,
                upvalues: vec![Upvalue::ParentUpvalue(1)],
                ..function(vec![Instruction::Return])
            },
        ] {
            assert_ne!(one_hash, hash_program(&with_function(vec![change])));
        }

        // Table order is semantic because MakeFunction stores stable table indices.  Referencing
        // each entry from the other function also exercises cyclic references without recursion.
        let first = function(vec![Instruction::MakeFunction(1), Instruction::Return]);
        let second = function(vec![Instruction::MakeFunction(0), Instruction::Return]);
        let cyclic = hash_program(&with_function(vec![first.clone(), second.clone()]));
        let reordered = hash_program(&with_function(vec![second, first]));
        assert_ne!(cyclic, reordered);
    }

    #[test]
    fn emits_stable_source_and_path_independent_symbol() {
        let program = program(
            vec![Constant::Number(42.0f64.to_bits())],
            vec![Instruction::Const(0), Instruction::Return],
            0,
        );
        let first = emit(&program).expect("program emits");
        let second = emit(&program).expect("program emits again");
        assert_eq!(first, second);
        assert!(first.symbol.starts_with("kiln_module_"));
        assert_eq!(first.symbol.len(), "kiln_module_".len() + 32);
        assert!(
            first.symbol["kiln_module_".len()..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        assert!(!first.source.contains("/Users/"));
        assert!(first.source.contains(&format!(
            "int {}(kiln_host *host, kiln_value *result)",
            first.symbol
        )));
    }

    #[test]
    fn module_linkage_is_deterministic_and_part_of_identity() {
        let body = program(
            vec![Constant::Number(1.0f64.to_bits())],
            vec![Instruction::LoadLocal(0), Instruction::Return],
            1,
        );
        let first = module(
            body.clone(),
            vec![kiln_ir::ImportBinding {
                source: "dep".into(),
                imported: "value".into(),
                local: "input".into(),
                local_slot: 0,
            }],
            Vec::new(),
        );
        let second = module(
            body,
            vec![kiln_ir::ImportBinding {
                source: "other-dep".into(),
                imported: "value".into(),
                local: "input".into(),
                local_slot: 0,
            }],
            Vec::new(),
        );
        let emitted = emit_module(&first).expect("module emits");
        assert_eq!(emitted, emit_module(&first).expect("module emits again"));
        assert_ne!(
            emitted.program_hash,
            emit_module(&second).unwrap().program_hash
        );
        assert!(emitted.source.contains("const kiln_value *imports"));
        assert!(emitted.source.contains("size_t import_count"));
    }

    #[test]
    fn callable_module_materializes_its_private_refusal_before_returning_to_the_graph() {
        let generated = emit_module_with_logical_path(
            &module(
                Program {
                    functions: vec![function(vec![Instruction::Return])],
                    ..program(
                        Vec::new(),
                        vec![Instruction::MakeFunction(0), Instruction::Return],
                        0,
                    )
                },
                Vec::new(),
                Vec::new(),
            ),
            "node_modules/provider/index.js",
        )
        .expect("callable module emits");

        assert!(generated.source.contains(
            "if (status == KILN_C99_CONTROL_ERROR)\n\
             \x20       *context->result = kiln_c99_control_diagnostic();"
        ));
        assert!(generated.source.contains(
            "if (status == KILN_C99_CONTROL_ERROR)\n\
             \x20           *result = kiln_c99_control_diagnostic();"
        ));
    }

    #[test]
    fn closure_instruction_refusals_materialize_before_crossing_module_boundaries() {
        let generated = emit_module_with_logical_path(
            &module(
                Program {
                    functions: vec![function(vec![
                        Instruction::LoadBuiltin(Builtin::Map),
                        Instruction::Call { argc: 0 },
                        Instruction::Return,
                    ])],
                    ..program(
                        Vec::new(),
                        vec![Instruction::MakeFunction(0), Instruction::Return],
                        0,
                    )
                },
                Vec::new(),
                Vec::new(),
            ),
            "node_modules/provider/index.js",
        )
        .expect("refusing closure module emits");

        assert!(generated.source.contains(
            "kiln_c99_note_refusal(\"node_modules/provider/index.js\", (size_t)1, \"Call\""
        ));
        assert!(
            generated
                .source
                .contains("(size_t)kiln_c99_diagnostic_active_depth); goto kiln_refusal;")
        );
    }

    #[test]
    fn logical_path_symbol_survives_body_edits_and_is_checkout_independent() {
        let first = module(
            program(
                vec![Constant::Number(1.0f64.to_bits())],
                vec![Instruction::Const(0), Instruction::Return],
                0,
            ),
            Vec::new(),
            Vec::new(),
        );
        let second = module(
            program(
                vec![Constant::Number(2.0f64.to_bits())],
                vec![Instruction::Const(0), Instruction::Return],
                0,
            ),
            Vec::new(),
            Vec::new(),
        );
        let one = emit_module_with_logical_path(&first, "src/dep.js").expect("module emits");
        let two = emit_module_with_logical_path(&second, "src/dep.js").expect("module emits");
        let other = emit_module_with_logical_path(&first, "src/other.js").expect("module emits");

        assert_eq!(one.symbol, two.symbol);
        assert_ne!(one.source, two.source);
        assert_ne!(one.symbol, other.symbol);
        assert!(!one.symbol.contains("/"));
        assert!(
            one.symbol
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        );
    }

    #[test]
    fn logical_path_symbol_rejects_non_normalized_or_absolute_input() {
        let module = module(
            program(Vec::new(), vec![Instruction::Return], 0),
            Vec::new(),
            Vec::new(),
        );
        for path in [
            "",
            "/src/dep.js",
            "../dep.js",
            "src/../dep.js",
            "src\\dep.js",
            "C:/dep.js",
        ] {
            assert!(
                matches!(
                    emit_module_with_logical_path(&module, path),
                    Err(EmitError::InvalidProgram { message }) if message.contains("logical module path")
                ),
                "path should be rejected: {path:?}"
            );
        }
    }

    #[test]
    fn rejects_invalid_module_linkage_metadata() {
        let body = program(Vec::new(), vec![Instruction::Return], 1);
        let out_of_bounds = module(
            body.clone(),
            vec![kiln_ir::ImportBinding {
                source: "dep".into(),
                imported: "value".into(),
                local: "input".into(),
                local_slot: 1,
            }],
            Vec::new(),
        );
        assert!(matches!(
            emit_module(&out_of_bounds),
            Err(EmitError::InvalidProgram { message }) if message.contains("import 0") && message.contains("out of bounds")
        ));
        let duplicate = module(
            body.clone(),
            vec![
                kiln_ir::ImportBinding {
                    source: "a".into(),
                    imported: "x".into(),
                    local: "x".into(),
                    local_slot: 0,
                },
                kiln_ir::ImportBinding {
                    source: "b".into(),
                    imported: "y".into(),
                    local: "y".into(),
                    local_slot: 0,
                },
            ],
            Vec::new(),
        );
        assert!(matches!(
            emit_module(&duplicate),
            Err(EmitError::InvalidProgram { message }) if message.contains("initialized more than once")
        ));
        let bad_export = module(
            body,
            Vec::new(),
            vec![kiln_ir::ExportBinding {
                exported: "value".into(),
                local_slot: 1,
            }],
        );
        assert!(matches!(
            emit_module(&bad_export),
            Err(EmitError::InvalidProgram { message }) if message.contains("export 0") && message.contains("out of bounds")
        ));
    }

    #[test]
    fn emitted_module_initializes_imports_and_publishes_exports() {
        let generated = emit_module(&module(
            program(
                vec![Constant::Number(2.0f64.to_bits())],
                vec![
                    Instruction::LoadLocal(0),
                    Instruction::Const(0),
                    Instruction::Binary(BinaryOp::Add),
                    Instruction::StoreLocal(1),
                    Instruction::LoadLocal(1),
                    Instruction::Return,
                ],
                2,
            ),
            vec![kiln_ir::ImportBinding {
                source: "dep".into(),
                imported: "value".into(),
                local: "input".into(),
                local_slot: 0,
            }],
            vec![kiln_ir::ExportBinding {
                exported: "answer".into(),
                local_slot: 1,
            }],
        ))
        .expect("module emits");
        let wrapper = format!(
            "int main(void) {{ kiln_value input = kiln_c99_number(40.0); kiln_value output; kiln_value result; if ({}(NULL, &result, &input, 1, &output, 1) != 0) return 2; if (output.tag != KILN_VALUE_NUMBER || output.bits != kiln_c99_number(42.0).bits) return 3; if ({}(NULL, &result, NULL, 0, &output, 1) != KILN_C99_INVALID_ARGUMENT) return 4; return 0; }}\n",
            generated.symbol, generated.symbol
        );
        let mut child = Command::new("cc")
            .args(["-std=c99", "-pedantic", "-x", "c", "-", "-lm", "-o"])
            .arg(std::env::temp_dir().join(format!("kiln-backend-c-module-{}", std::process::id())))
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a C compiler is available for backend tests");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(linked_source(&generated.source, &wrapper).as_bytes())
            .expect("source is accepted by cc stdin");
        let output_path =
            std::env::temp_dir().join(format!("kiln-backend-c-module-{}", std::process::id()));
        let compile = child.wait_with_output().expect("cc exits");
        assert!(
            compile.status.success(),
            "generated module C did not compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&output_path)
            .output()
            .expect("generated executable runs");
        let _ = std::fs::remove_file(output_path);
        assert!(
            run.status.success(),
            "generated module returned the wrong result"
        );
    }

    #[test]
    fn emits_every_scalar_constant_and_host_call_through_numeric_abi() {
        let program = program(
            vec![
                Constant::Undefined,
                Constant::Null,
                Constant::Boolean(true),
                Constant::Number((-0.0f64).to_bits()),
                Constant::String("a\0é".into()),
            ],
            vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::Const(2),
                Instruction::Const(3),
                Instruction::Const(4),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 5,
                },
                Instruction::Return,
            ],
            0,
        );
        let generated = emit(&program).expect("program emits").source;
        assert!(generated.contains("UINT16_C(1), UINT16_C(5)"));
        assert!(generated.contains("typedef int (*kiln_host_call_fn)"));
        assert!(generated.contains("char kiln_module_") || generated.contains("_constant_4"));
        assert!(!generated.contains("printf(") || generated.contains("snprintf("));
        assert!(generated.contains("\\x00\"\""));
    }

    #[test]
    fn generated_numeric_conversion_ignores_the_embedding_process_locale() {
        let program = program(
            vec![
                Constant::String("1.5".into()),
                Constant::Number(0.0_f64.to_bits()),
                Constant::String("value=".into()),
                Constant::Number(1.5_f64.to_bits()),
            ],
            vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::Binary(BinaryOp::Subtract),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Pop,
                Instruction::Const(2),
                Instruction::Const(3),
                Instruction::Binary(BinaryOp::Add),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Return,
            ],
            0,
        );
        let generated = emit(&program).expect("locale probe emits");
        assert!(
            generated
                .source
                .contains("kiln_string_to_number(value.ptr, value.len)")
        );
        // A *call*, matching the printf check below. Both names still appear in the runtime's
        // prose, where they are naming the things that must not be reached for.
        assert!(!generated.source.contains("strtod("));
        assert!(!generated.source.contains("kiln_c99_strtod_c"));
        assert!(generated.source.contains("KILN_NUMBER_FORMAT_VERSION 1"));
        assert!(generated.source.contains("KILN_NUMBER_PARSE_VERSION 1"));
        assert!(
            generated
                .source
                .contains("kiln_number_to_js_string(number, buffer, sizeof(buffer))")
        );
        assert!(!generated.source.contains("snprintf("));
        assert!(!generated.source.contains("setlocale("));
        let wrapper = format!(
            r#"
#include <locale.h>

static int calls = 0;

static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != UINT16_C(1) || argc != 1 || arguments == NULL || result == NULL)
        return KILN_C99_HOST_ERROR;
    if (calls == 0) {{
        if (arguments[0].tag != KILN_VALUE_NUMBER ||
            arguments[0].bits != UINT64_C({number_bits}))
            return KILN_C99_HOST_ERROR;
    }} else if (calls == 1) {{
        static const char expected[] = "value=1.5";
        if (arguments[0].tag != KILN_VALUE_STRING || arguments[0].ptr == NULL ||
            arguments[0].len != sizeof(expected) - 1 ||
            memcmp(arguments[0].ptr, expected, sizeof(expected) - 1) != 0)
            return KILN_C99_HOST_ERROR;
    }} else {{
        return KILN_C99_HOST_ERROR;
    }}
    ++calls;
    *result = kiln_c99_undefined();
    return KILN_C99_OK;
}}

int main(void) {{
    static const char *candidates[] = {{
        "de_DE.UTF-8", "de_DE.utf8", "de_DE",
        "fr_FR.UTF-8", "fr_FR.utf8", "fr_FR",
        "it_IT.UTF-8", "es_ES.UTF-8", "pt_BR.UTF-8"
    }};
    kiln_host host = {{ host_call, NULL }};
    kiln_value result;
    size_t index;
    int found = 0;
    for (index = 0; index < sizeof(candidates) / sizeof(candidates[0]); ++index) {{
        struct lconv *conventions;
        if (setlocale(LC_NUMERIC, candidates[index]) == NULL) continue;
        conventions = localeconv();
        if (conventions != NULL && strcmp(conventions->decimal_point, ",") == 0) {{
            found = 1;
            break;
        }}
    }}
    if (!found) return 77;
    if ({symbol}(&host, &result) != KILN_C99_OK) return 2;
    return calls == 2 ? 0 : 3;
}}
"#,
            number_bits = 1.5_f64.to_bits(),
            symbol = generated.symbol,
        );
        let output_path =
            std::env::temp_dir().join(format!("kiln-backend-c-locale-{}", std::process::id()));
        let mut child = Command::new("cc")
            .args(["-std=c11", "-x", "c", "-"])
            .arg("-lm")
            .arg("-o")
            .arg(&output_path)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a C compiler is available for locale test");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(linked_source(&generated.source, &wrapper).as_bytes())
            .expect("locale source is accepted by cc stdin");
        let compile = child.wait_with_output().expect("cc exits");
        assert!(
            compile.status.success(),
            "locale-independent generated C did not compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&output_path)
            .output()
            .expect("locale probe runs");
        let _ = std::fs::remove_file(output_path);
        if run.status.code() == Some(77) {
            eprintln!("skipping hostile-locale execution: no comma-decimal locale is installed");
            return;
        }
        assert!(
            run.status.success(),
            "generated numeric conversion inherited the embedding locale: {:?}",
            run.status.code()
        );
    }

    #[test]
    fn refuses_to_make_a_realm_by_name_rather_than_answering_from_its_only_one() {
        // The backend's global object is one copy built in C. A program that reached the host's
        // realm hook would get an object whose intrinsics were secretly this realm's, and every
        // cross-realm identity question would be answered confidently and wrongly -- so the hook
        // is refused where it is named, not where its answer is used.
        for code in [
            vec![
                Instruction::RequireGlobalBinding(0),
                Instruction::Pop,
                Instruction::Return,
            ],
            vec![
                Instruction::LoadGlobalBindingOrUndefined(0),
                Instruction::Pop,
                Instruction::Return,
            ],
        ] {
            let refused = emit(&program(
                vec![Constant::String(HOST_CREATE_REALM.to_owned())],
                code,
                0,
            ));
            let Err(EmitError::Unsupported { feature }) = refused else {
                panic!("the backend must refuse {HOST_CREATE_REALM}, not compile it");
            };
            assert!(
                feature.contains(HOST_CREATE_REALM),
                "the refusal must name what it refused, got {feature}"
            );
            let refused_eval = emit(&program(
                vec![Constant::String(HOST_EVAL_SCRIPT.to_owned())],
                vec![
                    Instruction::LoadGlobalBindingOrUndefined(0),
                    Instruction::Pop,
                    Instruction::Return,
                ],
                0,
            ));
            let Err(EmitError::Unsupported { feature }) = refused_eval else {
                panic!("the backend must refuse {HOST_EVAL_SCRIPT}, not compile it");
            };
            assert!(
                feature.contains(HOST_EVAL_SCRIPT),
                "the refusal must name what it refused, got {feature}"
            );
            // And each of the eight agent hooks, by its own name. A backend that quietly compiled
            // `$262AgentStart` into a no-op would answer every `Atomics.wait` "timed-out" while
            // the interpreter answered "ok", which is the divergence this refusal prevents.
            for hook in HOST_AGENT_HOOKS {
                let refused_agent = emit(&program(
                    vec![Constant::String(hook.to_owned())],
                    vec![
                        Instruction::LoadGlobalBindingOrUndefined(0),
                        Instruction::Pop,
                        Instruction::Return,
                    ],
                    0,
                ));
                let Err(EmitError::Unsupported { feature }) = refused_agent else {
                    panic!("the backend must refuse {hook}, not compile it");
                };
                assert!(
                    feature.contains(hook),
                    "the refusal must name what it refused, got {feature}"
                );
            }
            // `ShadowRealm` is refused for a sharper reason than the two hooks above: it is a
            // language built-in a program may name without any host granting it, and this
            // backend has one global object. Compiling it would answer `new ShadowRealm()` with
            // a realm whose intrinsics were secretly this one's.
            let refused_shadow = emit(&program(
                vec![Constant::String(SHADOW_REALM.to_owned())],
                vec![
                    Instruction::LoadGlobalBindingOrUndefined(0),
                    Instruction::Pop,
                    Instruction::Return,
                ],
                0,
            ));
            let Err(EmitError::Unsupported { feature }) = refused_shadow else {
                panic!("the backend must refuse {SHADOW_REALM}, not compile it");
            };
            assert!(
                feature.contains(SHADOW_REALM),
                "the refusal must name what it refused, got {feature}"
            );
            // The instruction that actually carries `ShadowRealm` in a compiled program. A name
            // this backend has no global for never reaches the lookup arms above: the compiler
            // emits the miss as `ThrowReference`, so refusing only the lookup let
            // `new ShadowRealm()` compile and answer `ReferenceError` at run time. That is a
            // claim about the *language* rather than about this backend, and the two engines
            // diverged on it.
            let refused_reference = emit(&program(
                vec![Constant::String(SHADOW_REALM.to_owned())],
                vec![Instruction::ThrowReference(0), Instruction::Return],
                0,
            ));
            let Err(EmitError::Unsupported { feature }) = refused_reference else {
                panic!("the backend must refuse a {SHADOW_REALM} reference, not compile it");
            };
            assert!(
                feature.contains(SHADOW_REALM),
                "the refusal must name what it refused, got {feature}"
            );
        }
        // An ordinary global of another name is untouched: this refuses one hook, not a class of
        // instruction the backend already supports.
        assert!(matches!(
            emit(&program(
                vec![Constant::String("Array".to_owned())],
                vec![
                    Instruction::LoadGlobalBindingOrUndefined(0),
                    Instruction::Pop,
                    Instruction::Return,
                ],
                0,
            )),
            Ok(_)
        ));
    }

    #[test]
    fn rejects_bad_versions_and_stack_shapes_explicitly() {
        let bad_version = Program {
            version: BYTECODE_VERSION + 1,
            ..program(Vec::new(), vec![Instruction::Return], 0)
        };
        assert!(matches!(
            emit(&bad_version),
            Err(EmitError::Unsupported { .. })
        ));
        let underflow = program(Vec::new(), vec![Instruction::Pop, Instruction::Return], 0);
        assert!(matches!(
            emit(&underflow),
            Err(EmitError::InvalidProgram { .. })
        ));
    }

    #[test]
    fn rejects_out_of_bounds_jumps_and_locals() {
        let jump = program(Vec::new(), vec![Instruction::Jump(9)], 0);
        assert!(matches!(emit(&jump), Err(EmitError::InvalidProgram { .. })));
        let local = program(
            Vec::new(),
            vec![Instruction::LoadLocal(0), Instruction::Return],
            0,
        );
        assert!(matches!(
            emit(&local),
            Err(EmitError::InvalidProgram { .. })
        ));
    }

    #[test]
    fn hash_changes_for_semantically_distinct_programs() {
        let one = emit(&program(
            vec![Constant::Number(1)],
            vec![Instruction::Const(0), Instruction::Return],
            0,
        ))
        .unwrap();
        let two = emit(&program(
            vec![Constant::Number(2)],
            vec![Instruction::Const(0), Instruction::Return],
            0,
        ))
        .unwrap();
        assert_ne!(one.program_hash, two.program_hash);
        assert_ne!(one.symbol, two.symbol);
    }

    #[test]
    fn generated_translation_unit_is_valid_c99() {
        let generated = emit(&program(
            vec![
                Constant::Number(2.0f64.to_bits()),
                Constant::String("hello\0world".into()),
            ],
            vec![
                Instruction::Const(1),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Const(0),
                Instruction::Binary(BinaryOp::Add),
                Instruction::Return,
            ],
            0,
        ))
        .expect("program emits");
        let mut child = Command::new("cc")
            .args(["-std=c99", "-pedantic", "-fsyntax-only", "-x", "c", "-"])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a C compiler is available for backend tests");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(generated.source.as_bytes())
            .expect("source is accepted by cc stdin");
        let result = child.wait_with_output().expect("cc exits");
        assert!(
            result.status.success(),
            "generated C did not compile:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn executable_promise_with_resolvers_exposes_and_settles_one_deferred() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("empty program emits runtime prelude");
        let wrapper = r#"
int main(void) {
    kiln_value method;
    kiln_value deferred;
    kiln_value promise;
    kiln_value resolve;
    kiln_value value = kiln_c99_number(42.0);
    kiln_value result;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_c99_get(NULL, kiln_c99_builtin(KILN_C99_BUILTIN_PROMISE),
                     "withResolvers", 13, &method, 0, &thrown) != KILN_C99_OK) return 1;
    if (method.tag != KILN_VALUE_METHOD ||
        method.bits != KILN_C99_METHOD_PROMISE_WITH_RESOLVERS) return 2;
    if (kiln_c99_call(NULL, &deferred, method, kiln_c99_builtin(KILN_C99_BUILTIN_PROMISE),
                      NULL, 0, 0, &thrown) != KILN_C99_OK) return 3;
    if (kiln_c99_get(NULL, deferred, "promise", 7, &promise, 0, &thrown) != KILN_C99_OK ||
        promise.tag != KILN_VALUE_PROMISE) return 4;
    if (kiln_c99_get(NULL, deferred, "resolve", 7, &resolve, 0, &thrown) != KILN_C99_OK ||
        resolve.tag != KILN_VALUE_SETTLER) return 5;
    if (kiln_c99_call(NULL, &result, resolve, kiln_c99_undefined(), &value, 1, 0,
                      &thrown) != KILN_C99_OK) return 6;
    if (kiln_c99_promise_of(promise)->state != KILN_C99_FULFILLED ||
        kiln_c99_as_number(kiln_c99_promise_of(promise)->value) != 42.0) return 7;
    return 0;
}
"#;
        let run = run_with_main(&generated, wrapper, "promise-with-resolvers");
        assert!(
            run.status.success(),
            "Promise.withResolvers failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_native_callables_have_isolated_static_property_stores() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("empty program emits runtime prelude");
        let wrapper = r#"
int main(void) {
    kiln_value stream = kiln_c99_method(KILN_C99_METHOD_NODE_STREAM);
    kiln_value readable = kiln_c99_method(KILN_C99_METHOD_NODE_READABLE);
    kiln_value buffer = kiln_c99_builtin(KILN_C99_BUILTIN_BUFFER);
    kiln_value constants;
    kiln_value read;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_c99_put(NULL, stream, "Readable", 8, readable, 0, &thrown) != KILN_C99_OK) return 1;
    if (kiln_c99_new_object(&constants) != KILN_C99_OK) return 2;
    if (kiln_c99_put(NULL, buffer, "constants", 9, constants, 0, &thrown) != KILN_C99_OK) return 3;
    if (kiln_c99_get(NULL, stream, "Readable", 8, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_METHOD || read.bits != KILN_C99_METHOD_NODE_READABLE) return 4;
    if (kiln_c99_get(NULL, buffer, "constants", 9, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_OBJECT || read.bits != constants.bits) return 5;
    if (kiln_c99_get(NULL, stream, "constants", 9, &read, 0, &thrown) == KILN_C99_OK) return 6;
    return 0;
}
"#;
        let run = run_with_main(&generated, wrapper, "native-callable-statics");
        assert!(
            run.status.success(),
            "native callable statics failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_bound_callables_have_isolated_own_property_stores() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("empty program emits runtime prelude");
        let wrapper = r#"
int main(void) {
    kiln_value target = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_NOOP);
    kiln_value first;
    kiln_value second;
    kiln_value read;
    kiln_value thrown = kiln_c99_undefined();
    kiln_value marker = kiln_c99_copy_text("first", 5);
    if (kiln_c99_bind_with(&first, target, kiln_c99_undefined(), NULL, 0) != KILN_C99_OK ||
        kiln_c99_bind_with(&second, target, kiln_c99_undefined(), NULL, 0) != KILN_C99_OK)
        return 1;
    if (kiln_c99_put(NULL, first, "unwrappedName", 13, marker, 1, &thrown) != KILN_C99_OK)
        return 2;
    if (kiln_c99_get(NULL, first, "unwrappedName", 13, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_STRING || read.len != 5 || memcmp(read.ptr, "first", 5) != 0)
        return 3;
    if (kiln_c99_get(NULL, second, "unwrappedName", 13, &read, 0, &thrown) == KILN_C99_OK)
        return 4;
    if (kiln_c99_get(NULL, target, "unwrappedName", 13, &read, 0, &thrown) == KILN_C99_OK)
        return 5;
    return 0;
}
"#;
        let run = run_with_main(&generated, wrapper, "bound-callable-own-properties");
        assert!(
            run.status.success(),
            "bound callable own properties failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_array_buffer_is_view_recognizes_native_views() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("empty program emits runtime prelude");
        let wrapper = r#"
int main(void) {
    kiln_value method;
    kiln_value view;
    kiln_value buffer;
    kiln_value result;
    kiln_value argument;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_c99_get(NULL, kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER),
                     "isView", 6, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD ||
        method.bits != KILN_C99_METHOD_ARRAY_BUFFER_IS_VIEW) return 1;
    if (kiln_c99_new_typed_buffer(&buffer, 8) != KILN_C99_OK) return 2;
    if (kiln_c99_new_typed_array(&view, KILN_TYPED_UINT8, buffer, 0, 8, 1,
                                 &thrown) != KILN_C99_OK) return 3;
    argument = view;
    if (kiln_c99_call(NULL, &result, method,
                      kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER),
                      &argument, 1, 0, &thrown) != KILN_C99_OK ||
        result.tag != KILN_VALUE_BOOLEAN || !result.bits) return 4;
    argument = buffer;
    if (kiln_c99_call(NULL, &result, method,
                      kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER),
                      &argument, 1, 0, &thrown) != KILN_C99_OK ||
        result.tag != KILN_VALUE_BOOLEAN || result.bits) return 5;
    argument = kiln_c99_undefined();
    if (kiln_c99_call(NULL, &result, method,
                      kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER),
                      &argument, 1, 0, &thrown) != KILN_C99_OK ||
        result.tag != KILN_VALUE_BOOLEAN || result.bits) return 6;
    return 0;
}
"#;
        let run = run_with_main(&generated, wrapper, "array-buffer-is-view");
        assert!(
            run.status.success(),
            "ArrayBuffer.isView failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_typed_array_tag_is_a_shared_prototype_accessor() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("empty program emits runtime prelude");
        let wrapper = r#"
int main(void) {
    kiln_value descriptor;
    kiln_value getter;
    kiln_value view;
    kiln_value tag;
    kiln_value thrown = kiln_c99_undefined();
    int found = 0;
    if (kiln_c99_describe_property(NULL,
                                   kiln_c99_builtin(KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE),
                                   "@@toStringTag", 13, &descriptor, &found, 0, &thrown) !=
            KILN_C99_OK || !found) return 1;
    if (kiln_c99_get(NULL, descriptor, "get", 3, &getter, 0, &thrown) != KILN_C99_OK ||
        getter.tag != KILN_VALUE_METHOD ||
        getter.bits != KILN_C99_METHOD_TYPED_TO_STRING_TAG) return 2;
    if (kiln_c99_new_typed_array(&view, KILN_TYPED_INT8, kiln_c99_undefined(), 0, 0, 0,
                                 &thrown) != KILN_C99_OK) return 3;
    if (kiln_c99_call(NULL, &tag, getter, view, NULL, 0, 0, &thrown) != KILN_C99_OK ||
        tag.tag != KILN_VALUE_STRING || tag.len != 9 ||
        memcmp(tag.ptr, "Int8Array", 9) != 0) return 4;
    return 0;
}
"#;
        let run = run_with_main(&generated, wrapper, "typed-array-tag-accessor");
        assert!(
            run.status.success(),
            "typed-array tag accessor failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_accessor_definition_bypasses_an_inherited_setter() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("empty program emits runtime prelude");
        let wrapper = r#"
int main(void) {
    kiln_value parent;
    kiln_value child;
    kiln_value inherited = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_NOOP);
    kiln_value own = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_FORMAT);
    kiln_value descriptor;
    kiln_value read;
    kiln_value thrown = kiln_c99_undefined();
    int found = 0;
    if (kiln_c99_new_object(&parent) != KILN_C99_OK ||
        kiln_c99_new_object(&child) != KILN_C99_OK) return 1;
    if (kiln_c99_define_accessor(parent, "value", 5, kiln_c99_undefined(), inherited, 0) !=
        KILN_C99_OK) return 2;
    if (kiln_c99_set_prototype(child, parent) != KILN_C99_OK) return 3;
    if (kiln_c99_define_accessor(child, "value", 5, own, kiln_c99_undefined(), 1) !=
        KILN_C99_OK) return 4;
    if (kiln_c99_describe_property(NULL, child, "value", 5, &descriptor, &found, 0, &thrown) !=
            KILN_C99_OK || !found) return 5;
    if (kiln_c99_get(NULL, descriptor, "get", 3, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_METHOD || read.bits != own.bits) return 6;
    if (kiln_c99_get(NULL, descriptor, "set", 3, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_UNDEFINED) return 7;
    if (kiln_c99_get(NULL, descriptor, "enumerable", 10, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_BOOLEAN || read.bits != 1) return 8;
    return 0;
}
"#;
        let run = run_with_main(&generated, wrapper, "accessor-definition-own-slot");
        assert!(
            run.status.success(),
            "accessor definition failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_array_from_maps_an_array_like_object() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0))
            .expect("empty program emits runtime prelude");
        let wrapper = r#"
int main(void) {
    kiln_value source;
    kiln_value method = kiln_c99_method(KILN_C99_METHOD_ARRAY_FROM);
    kiln_value mapper = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_NOOP);
    kiln_value arguments[3];
    kiln_value result;
    kiln_value value;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_c99_new_object(&source) != KILN_C99_OK) return 1;
    if (kiln_c99_put(NULL, source, "length", 6, kiln_c99_number(2), 0, &thrown) != KILN_C99_OK ||
        kiln_c99_put(NULL, source, "0", 1, kiln_c99_copy_text("a", 1), 0, &thrown) != KILN_C99_OK)
        return 2;
    arguments[0] = source;
    arguments[1] = mapper;
    arguments[2] = kiln_c99_undefined();
    if (kiln_c99_call(NULL, &result, method, kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY),
                      arguments, 3, 0, &thrown) != KILN_C99_OK) return 3;
    if (result.tag != KILN_VALUE_ARRAY || kiln_c99_array_of(result)->count != 2) return 4;
    value = kiln_c99_array_of(result)->elements[0];
    if (value.tag != KILN_VALUE_UNDEFINED) return 5;
    value = kiln_c99_array_of(result)->elements[1];
    if (value.tag != KILN_VALUE_UNDEFINED) return 6;
    return 0;
}
"#;
        let run = run_with_main(&generated, wrapper, "array-from-array-like");
        assert!(
            run.status.success(),
            "Array.from array-like failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_scalar_operators_match_javascript() {
        fn push_constant(constants: &mut Vec<Constant>, value: Constant) -> u32 {
            let index = constants.len() as u32;
            constants.push(value);
            index
        }

        fn check_unary(
            constants: &mut Vec<Constant>,
            code: &mut Vec<Instruction>,
            failures: &mut Vec<usize>,
            operator: UnaryOp,
            input: Constant,
            expected: Constant,
        ) {
            let input = push_constant(constants, input);
            let expected = push_constant(constants, expected);
            code.extend([
                Instruction::Const(input),
                Instruction::Unary(operator),
                Instruction::Const(expected),
                Instruction::Binary(BinaryOp::StrictEqual),
            ]);
            failures.push(code.len());
            code.push(Instruction::JumpIfFalse(0));
        }

        fn check_binary(
            constants: &mut Vec<Constant>,
            code: &mut Vec<Instruction>,
            failures: &mut Vec<usize>,
            operator: BinaryOp,
            left: Constant,
            right: Constant,
            expected: Constant,
        ) {
            let left = push_constant(constants, left);
            let right = push_constant(constants, right);
            let expected = push_constant(constants, expected);
            code.extend([
                Instruction::Const(left),
                Instruction::Const(right),
                Instruction::Binary(operator),
                Instruction::Const(expected),
                Instruction::Binary(BinaryOp::StrictEqual),
            ]);
            failures.push(code.len());
            code.push(Instruction::JumpIfFalse(0));
        }

        let number = |value: f64| Constant::Number(value.to_bits());
        let mut constants = Vec::new();
        let mut code = Vec::new();
        let mut failures = Vec::new();

        for (input, expected) in [
            (Constant::Undefined, "undefined"),
            (Constant::Null, "object"),
            (Constant::Boolean(true), "boolean"),
            (number(1.0), "number"),
            (Constant::String("x".into()), "string"),
        ] {
            check_unary(
                &mut constants,
                &mut code,
                &mut failures,
                UnaryOp::TypeOf,
                input,
                Constant::String(expected.into()),
            );
        }
        check_unary(
            &mut constants,
            &mut code,
            &mut failures,
            UnaryOp::BitwiseNot,
            number(5.0),
            number(-6.0),
        );
        check_unary(
            &mut constants,
            &mut code,
            &mut failures,
            UnaryOp::BitwiseNot,
            number(4_294_967_296.0),
            number(-1.0),
        );
        check_unary(
            &mut constants,
            &mut code,
            &mut failures,
            UnaryOp::Void,
            number(42.0),
            Constant::Undefined,
        );

        for (left, right, expected) in [
            (Constant::Null, Constant::Undefined, true),
            (number(0.0), Constant::Boolean(false), true),
            (Constant::String("42".into()), number(42.0), true),
            (Constant::String(String::new()), number(0.0), true),
            (Constant::Null, number(0.0), false),
        ] {
            check_binary(
                &mut constants,
                &mut code,
                &mut failures,
                BinaryOp::LooseEqual,
                left,
                right,
                Constant::Boolean(expected),
            );
        }
        check_binary(
            &mut constants,
            &mut code,
            &mut failures,
            BinaryOp::LooseNotEqual,
            number(f64::NAN),
            number(f64::NAN),
            Constant::Boolean(true),
        );

        for (operator, left, right, expected) in [
            (BinaryOp::BitwiseAnd, 5.0, 3.0, 1.0),
            (BinaryOp::BitwiseOr, 5.0, 2.0, 7.0),
            (BinaryOp::BitwiseXor, 5.0, 3.0, 6.0),
            (BinaryOp::ShiftLeft, 1.0, 31.0, -2_147_483_648.0),
            (BinaryOp::ShiftLeft, 1.0, 32.0, 1.0),
            (BinaryOp::ShiftRight, -1.0, 1.0, -1.0),
            (BinaryOp::UnsignedShiftRight, -1.0, 1.0, 2_147_483_647.0),
            (BinaryOp::BitwiseOr, 4_294_967_296.0, 0.0, 0.0),
            (BinaryOp::BitwiseOr, -1.9, 0.0, -1.0),
            (BinaryOp::BitwiseOr, f64::INFINITY, 0.0, 0.0),
        ] {
            check_binary(
                &mut constants,
                &mut code,
                &mut failures,
                operator,
                number(left),
                number(right),
                number(expected),
            );
        }

        let success = push_constant(&mut constants, Constant::Boolean(true));
        code.extend([Instruction::Const(success), Instruction::Return]);
        let failure = code.len() as u32;
        let failed = push_constant(&mut constants, Constant::Boolean(false));
        code.extend([Instruction::Const(failed), Instruction::Return]);
        for instruction in failures {
            code[instruction] = Instruction::JumpIfFalse(failure);
        }

        let generated = emit(&program(constants, code, 0)).expect("scalar operators emit");
        let executable = std::env::temp_dir().join(format!(
            "kiln-backend-c-operators-{}-{}",
            std::process::id(),
            generated.program_hash
        ));
        let wrapper = format!(
            "int main(void) {{ kiln_value result; if ({}(NULL, &result) != 0) return 2; return !(result.tag == KILN_VALUE_BOOLEAN && result.bits == 1); }}\n",
            generated.symbol
        );
        let mut child = Command::new("cc")
            .args(["-std=c99", "-pedantic", "-x", "c", "-", "-lm", "-o"])
            .arg(&executable)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a C compiler is available for backend tests");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(linked_source(&generated.source, &wrapper).as_bytes())
            .expect("operator source is accepted by cc stdin");
        let compile = child.wait_with_output().expect("cc exits");
        assert!(
            compile.status.success(),
            "generated operator C did not compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&executable)
            .output()
            .expect("generated operator executable runs");
        let _ = std::fs::remove_file(&executable);
        assert!(
            run.status.success(),
            "generated scalar operator returned the wrong result"
        );
    }

    #[test]
    fn executable_concat_accepts_a_null_pointer_for_an_empty_host_string() {
        let generated = emit(&program(
            vec![Constant::String("x".into())],
            vec![
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 0,
                },
                Instruction::Const(0),
                Instruction::Binary(BinaryOp::Add),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Return,
            ],
            0,
        ))
        .expect("empty host string program emits");
        let executable = std::env::temp_dir().join(format!(
            "kiln-backend-c-empty-host-string-{}-{}",
            std::process::id(),
            generated.program_hash
        ));
        let wrapper = format!(
            r#"
static int calls = 0;

static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != UINT16_C(1) || result == NULL) return KILN_C99_HOST_ERROR;
    if (calls == 0 && argc == 0) {{
        *result = kiln_c99_value(KILN_VALUE_STRING, 0, NULL, 0);
    }} else if (calls == 1 && argc == 1 && arguments != NULL &&
               arguments[0].tag == KILN_VALUE_STRING && arguments[0].len == 1 &&
               arguments[0].ptr != NULL && arguments[0].ptr[0] == 'x') {{
        *result = kiln_c99_undefined();
    }} else {{
        return KILN_C99_HOST_ERROR;
    }}
    ++calls;
    return KILN_C99_OK;
}}

int main(void) {{
    kiln_host host = {{host_call, NULL}};
    kiln_value result;
    if ({symbol}(&host, &result) != KILN_C99_OK) return 2;
    return calls == 2 ? 0 : 3;
}}
"#,
            symbol = generated.symbol
        );
        let mut child = Command::new("cc")
            .args(["-std=c99", "-pedantic", "-x", "c", "-", "-lm", "-o"])
            .arg(&executable)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a C compiler is available for backend tests");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(linked_source(&generated.source, &wrapper).as_bytes())
            .expect("empty host string source is accepted by cc stdin");
        let compile = child.wait_with_output().expect("cc exits");
        assert!(
            compile.status.success(),
            "generated empty host string C did not compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&executable)
            .output()
            .expect("generated empty host string executable runs");
        let _ = std::fs::remove_file(&executable);
        assert!(run.status.success(), "empty host string concat failed");
    }

    /// Every binary operator emits now. The rule the two refused ones stood for still holds and
    /// is worth a test of its own: an instruction this backend cannot do must say so rather than
    /// emit a silent undefined, which is what `unreachable_unsupported_instructions` covers.
    #[test]
    fn the_reference_operators_emit_rather_than_answering_undefined() {
        for operator in [BinaryOp::In, BinaryOp::InstanceOf] {
            let result = emit(&program(
                vec![Constant::Null],
                vec![
                    Instruction::Const(0),
                    Instruction::Const(0),
                    Instruction::Binary(operator),
                    Instruction::Return,
                ],
                0,
            ));
            assert!(result.is_ok(), "{operator:?} should emit");
        }
    }

    #[test]
    fn unreachable_weak_map_builtin_is_still_rendered_consistently() {
        let result = emit(&program(
            vec![Constant::Boolean(true)],
            vec![
                Instruction::Jump(2),
                Instruction::LoadBuiltin(kiln_ir::Builtin::WeakMap),
                Instruction::Const(0),
                Instruction::Return,
            ],
            0,
        ))
        .expect("WeakMap is a native builtin");
        assert!(result.source.contains("KILN_C99_BUILTIN_WEAK_MAP"));
    }

    /// Compile one emitted unit with a `main` of the caller's choosing and report its exit code.
    fn run_with_main(generated: &CModule, wrapper: &str, label: &str) -> std::process::Output {
        let executable = std::env::temp_dir().join(format!(
            "kiln-backend-c-{label}-{}-{}",
            std::process::id(),
            generated.program_hash
        ));
        let mut child = Command::new("cc")
            .args(["-std=c99", "-pedantic", "-x", "c", "-", "-lm", "-o"])
            .arg(&executable)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a C compiler is available for backend tests");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(linked_source(&generated.source, wrapper).as_bytes())
            .expect("source is accepted by cc stdin");
        let compile = child.wait_with_output().expect("cc exits");
        assert!(
            compile.status.success(),
            "generated C did not compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&executable)
            .output()
            .expect("generated executable runs");
        let _ = std::fs::remove_file(&executable);
        run
    }

    #[test]
    fn borrowed_array_map_and_filter_keep_generic_typed_array_semantics() {
        let callback = FunctionProto {
            /* Return the callback's third argument, the receiver. This makes a snapshot receiver
             * observably different from the original typed view. */
            code: vec![
                Instruction::LoadArguments,
                Instruction::Const(8),
                Instruction::GetComputed,
                Instruction::Return,
            ],
            ..function(Vec::new())
        };
        let constants = vec![
            Constant::Number(1.0f64.to_bits()),
            Constant::Number(2.0f64.to_bits()),
            Constant::String("prototype".into()),
            Constant::String("map".into()),
            Constant::String("filter".into()),
            Constant::String("call".into()),
            Constant::String("constructor".into()),
            Constant::String("length".into()),
            Constant::String("2".into()),
            Constant::Number(0.0f64.to_bits()),
            Constant::Boolean(true),
            Constant::Boolean(false),
        ];
        let mut code = vec![
            // typed = new Uint8Array([1, 2])
            Instruction::LoadBuiltin(Builtin::Uint8Array),
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::NewArray { count: 2 },
            Instruction::New { argc: 1 },
            Instruction::StoreLocal(0),
            // mapped = Array.prototype.map.call(typed, callback)
            Instruction::LoadBuiltin(Builtin::Array),
            Instruction::GetProperty(2),
            Instruction::GetProperty(3),
            Instruction::LoadLocal(0),
            Instruction::MakeFunction(0),
            Instruction::CallMethod { name: 5, argc: 2 },
            Instruction::StoreLocal(1),
            // The borrowed method returns an ordinary Array and passes the original typed view.
            Instruction::LoadLocal(1),
            Instruction::GetProperty(6),
            Instruction::LoadBuiltin(Builtin::Array),
            Instruction::Binary(BinaryOp::StrictEqual),
        ];
        let mut failed = Vec::new();
        failed.push(code.len());
        code.push(Instruction::JumpIfFalse(0));
        code.extend([
            Instruction::LoadLocal(1),
            Instruction::Const(9),
            Instruction::GetComputed,
            Instruction::LoadLocal(0),
            Instruction::Binary(BinaryOp::StrictEqual),
        ]);
        failed.push(code.len());
        code.push(Instruction::JumpIfFalse(0));
        code.extend([
            // filtered = Array.prototype.filter.call(typed, callback)
            Instruction::LoadBuiltin(Builtin::Array),
            Instruction::GetProperty(2),
            Instruction::GetProperty(4),
            Instruction::LoadLocal(0),
            Instruction::MakeFunction(0),
            Instruction::CallMethod { name: 5, argc: 2 },
            Instruction::StoreLocal(2),
            Instruction::LoadLocal(2),
            Instruction::GetProperty(6),
            Instruction::LoadBuiltin(Builtin::Array),
            Instruction::Binary(BinaryOp::StrictEqual),
        ]);
        failed.push(code.len());
        code.push(Instruction::JumpIfFalse(0));
        code.extend([
            Instruction::LoadLocal(2),
            Instruction::GetProperty(7),
            Instruction::Const(1),
            Instruction::Binary(BinaryOp::StrictEqual),
        ]);
        failed.push(code.len());
        code.push(Instruction::JumpIfFalse(0));
        code.extend([Instruction::Const(10), Instruction::Return]);
        let failure = code.len() as u32;
        code.extend([Instruction::Const(11), Instruction::Return]);
        for jump in failed {
            code[jump] = Instruction::JumpIfFalse(failure);
        }

        let generated = emit(&Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants,
            code,
            local_count: 3,
            functions: vec![callback],
        })
        .expect("borrowed typed-array methods emit");
        let wrapper = format!(
            "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 1; }}\n",
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "typed-array-borrowed-generic");
        assert!(
            run.status.success(),
            "borrowed Array.prototype map/filter changed typed-array semantics (exit {:?})",
            run.status.code()
        );
    }

    /// A closure over a module-level binding, which is the shape every counter has: the module
    /// body and the function must write through one box, and the module must still read the
    /// current value out of the slot afterwards.
    #[test]
    fn executable_closure_shares_a_cell_with_the_module_body() {
        let bump = FunctionProto {
            prologue_end: 0,
            local_count: 0,
            upvalues: vec![Upvalue::ParentLocal(0)],
            code: vec![
                Instruction::LoadUpvalue(0),
                Instruction::Const(1),
                Instruction::Binary(BinaryOp::Add),
                Instruction::StoreUpvalue(0),
                Instruction::LoadUpvalue(0),
                Instruction::Return,
            ],
            ..function(Vec::new())
        };
        let generated = emit(&Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![
                Constant::Number(0f64.to_bits()),
                Constant::Number(1f64.to_bits()),
            ],
            code: vec![
                Instruction::NewCell(0),
                Instruction::Const(0),
                Instruction::StoreCell(0),
                Instruction::MakeFunction(0),
                Instruction::StoreLocal(1),
                Instruction::LoadLocal(1),
                Instruction::Call { argc: 0 },
                Instruction::Pop,
                Instruction::LoadLocal(1),
                Instruction::Call { argc: 0 },
                Instruction::Pop,
                // The module reads the shared box, which must now hold 2.
                Instruction::LoadCell(0),
                Instruction::Return,
            ],
            local_count: 2,
            functions: vec![bump],
        })
        .expect("program emits");
        let wrapper = format!(
            "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; if (result.tag != KILN_VALUE_NUMBER) return 3; return kiln_c99_as_number(result) == 2.0 ? 0 : 4; }}\n",
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "closure-cell");
        assert!(
            run.status.success(),
            "a closure and its module body did not share one cell (exit {:?})",
            run.status.code()
        );
    }

    /// An arrow has no call-time receiver. It snapshots the receiver of the frame that creates
    /// it, even when a later plain call supplies `undefined` as `this`.
    #[test]
    fn executable_arrow_captures_its_defining_receiver() {
        let arrow = FunctionProto {
            prologue_end: 0,
            arrow: true,
            code: vec![Instruction::LoadThis, Instruction::Return],
            ..function(Vec::new())
        };
        let maker = FunctionProto {
            prologue_end: 0,
            code: vec![Instruction::MakeFunction(1), Instruction::Return],
            ..function(Vec::new())
        };
        let generated = emit(&Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::String("make".into())],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::StoreLocal(0),
                Instruction::NewObject { count: 0 },
                Instruction::StoreLocal(1),
                Instruction::LoadLocal(1),
                Instruction::LoadLocal(0),
                Instruction::SetProperty(0),
                Instruction::Pop,
                Instruction::LoadLocal(1),
                Instruction::CallMethod { name: 0, argc: 0 },
                Instruction::StoreLocal(2),
                Instruction::LoadLocal(2),
                Instruction::Call { argc: 0 },
                Instruction::LoadLocal(1),
                Instruction::Binary(BinaryOp::StrictEqual),
                Instruction::Return,
            ],
            local_count: 3,
            functions: vec![maker, arrow],
        })
        .expect("arrow receiver program emits");
        let wrapper = format!(
            "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits == 1 ? 0 : 3; }}\n",
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "arrow-receiver");
        assert!(
            run.status.success(),
            "an arrow observed its call-time receiver instead of its defining receiver (exit {:?})",
            run.status.code()
        );
    }

    /// Recursion is bounded rather than left to exhaust the C stack, which aborts the process
    /// instead of reporting anything. It reports as a *thrown* `RangeError` rather than a
    /// status, because that is what a real engine raises and what a program that recurses on its
    /// input has to be able to catch.
    #[test]
    fn executable_runaway_recursion_reports_rather_than_crashing() {
        let forever = FunctionProto {
            prologue_end: 0,
            local_count: 0,
            upvalues: vec![Upvalue::ParentLocal(0)],
            code: vec![
                Instruction::LoadUpvalue(0),
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            ..function(Vec::new())
        };
        let generated = emit(&Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![
                Instruction::NewCell(0),
                Instruction::MakeFunction(0),
                Instruction::StoreCell(0),
                Instruction::LoadCell(0),
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            local_count: 1,
            functions: vec![forever],
        })
        .expect("program emits");
        let wrapper = format!(
            "int main(void) {{ kiln_value result; return {}(NULL, &result) == KILN_C99_THROWN ? 0 : 1; }}\n",
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "depth");
        assert!(
            run.status.success(),
            "unbounded recursion did not report a thrown RangeError (exit {:?})",
            run.status.code()
        );
    }

    #[test]
    fn executable_relational_comparison_uses_utf16_code_units() {
        let generated = emit(&program(
            vec![
                Constant::String("😀".into()),
                Constant::String("\u{e000}".into()),
                Constant::String("é".into()),
                Constant::String("e".into()),
                Constant::Boolean(true),
                Constant::Boolean(false),
            ],
            vec![
                // U+1F600 is encoded as D83D DE00, so it sorts before U+E000 in UTF-16.
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::Binary(BinaryOp::LessThan),
                Instruction::JumpIfFalse(11),
                Instruction::Const(2),
                Instruction::Const(3),
                Instruction::Binary(BinaryOp::GreaterThan),
                Instruction::JumpIfFalse(11),
                Instruction::Const(4),
                Instruction::Return,
                Instruction::Const(5),
                Instruction::Return,
            ],
            0,
        ))
        .expect("program emits");
        let executable = std::env::temp_dir().join(format!(
            "kiln-backend-c-utf16-{}-{}",
            std::process::id(),
            generated.program_hash
        ));
        let wrapper = format!(
            "int main(void) {{ kiln_value result; if ({}(NULL, &result) != 0) return 2; return !(result.tag == KILN_VALUE_BOOLEAN && result.bits == 1); }}\n",
            generated.symbol
        );
        let mut child = Command::new("cc")
            .args(["-std=c99", "-pedantic", "-x", "c", "-", "-lm", "-o"])
            .arg(&executable)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a C compiler is available for backend tests");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(linked_source(&generated.source, &wrapper).as_bytes())
            .expect("source is accepted by cc stdin");
        let compile = child.wait_with_output().expect("cc exits");
        assert!(
            compile.status.success(),
            "generated C did not compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&executable)
            .output()
            .expect("generated executable runs");
        let _ = std::fs::remove_file(&executable);
        assert!(
            run.status.success(),
            "UTF-16 relational comparison returned the wrong result"
        );
    }
    #[cfg(any())]
    mod pre_typed_array_tests {

        #[test]
        fn executable_array_string_iterator_and_named_enumeration_are_distinct() {
            let generated = emit(&program(
                vec![Constant::String("@@iterator".into())],
                vec![
                    Instruction::NewArray { count: 0 },
                    Instruction::Const(0),
                    Instruction::GetComputed,
                    Instruction::Return,
                ],
                0,
            ))
            .expect("array string iterator probe emits");
            let wrapper = format!(
                r#"
int main(void) {{
    kiln_value result;
    if ({symbol}(NULL, &result) != KILN_C99_OK) return 2;
    return result.tag == KILN_VALUE_UNDEFINED ? 0 : 1;
}}
"#,
                symbol = generated.symbol
            );
            let run = run_with_main(&generated, &wrapper, "array-string-iterator");
            assert!(
                run.status.success(),
                "an absent ordinary @@iterator key must not expose Symbol.iterator"
            );

            let constants = vec![
                Constant::Number(0.0f64.to_bits()),
                Constant::Number(1.0f64.to_bits()),
                Constant::Number(2.0f64.to_bits()),
                Constant::Number(3.0f64.to_bits()),
                Constant::String("a".into()),
                Constant::String("b".into()),
                Constant::String("keys".into()),
                Constant::String("values".into()),
                Constant::String("entries".into()),
            ];
            let array = |code: &mut Vec<Instruction>| {
                code.extend([
                    Instruction::Const(1),
                    Instruction::NewArray { count: 1 },
                    Instruction::Dup,
                    Instruction::Const(5),
                    Instruction::Const(2),
                    Instruction::SetComputed,
                    Instruction::Pop,
                    Instruction::Dup,
                    Instruction::Const(4),
                    Instruction::Const(3),
                    Instruction::SetComputed,
                    Instruction::Pop,
                ]);
            };
            let probes = [
                (6, 2, 4, "keys"),    // Object.keys(array)[2] == "a".
                (7, 2, 3, "values"),  // Object.values(array)[2] == 3.
                (8, 1, 5, "entries"), // Object.entries(array)[1][0] == "b".
            ];
            for (method, index, expected, label) in probes {
                let mut code = vec![Instruction::LoadBuiltin(kiln_ir::Builtin::Object)];
                array(&mut code);
                code.push(Instruction::CallMethod {
                    name: method,
                    argc: 1,
                });
                code.push(Instruction::Const(index));
                code.push(Instruction::GetComputed);
                if method == 8 {
                    code.push(Instruction::Const(0));
                    code.push(Instruction::GetComputed);
                    code.push(Instruction::Const(5));
                } else {
                    code.push(Instruction::Const(expected));
                }
                code.extend([
                    Instruction::Binary(BinaryOp::StrictEqual),
                    Instruction::Return,
                ]);
                let generated = emit(&program(constants.clone(), code, 0))
                    .expect("array enumeration probe emits");
                let wrapper = format!(
                    "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 1; }}\n",
                    generated.symbol
                );
                let run = run_with_main(&generated, &wrapper, label);
                assert!(
                    run.status.success(),
                    "array {label} omitted named string properties"
                );
            }

            let mut code = Vec::new();
            array(&mut code);
            code.push(Instruction::EnumerateKeys);
            code.push(Instruction::Const(2));
            code.push(Instruction::GetComputed);
            code.push(Instruction::Const(4));
            code.extend([
                Instruction::Binary(BinaryOp::StrictEqual),
                Instruction::Return,
            ]);
            let generated = emit(&program(constants, code, 0)).expect("for-in probe emits");
            let wrapper = format!(
                "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 1; }}\n",
                generated.symbol
            );
            let run = run_with_main(&generated, &wrapper, "array-for-in");
            assert!(
                run.status.success(),
                "for-in omitted named string properties"
            );
        }

        #[test]
        fn executable_inspection_skips_symbol_properties_without_separators() {
            let generated = emit(&program(
                vec![
                    Constant::String("iterator".into()),
                    Constant::Number(1.0f64.to_bits()),
                    Constant::String("a".into()),
                    Constant::Number(2.0f64.to_bits()),
                ],
                vec![
                    Instruction::LoadBuiltin(kiln_ir::Builtin::Symbol),
                    Instruction::GetProperty(0),
                    Instruction::Const(1),
                    Instruction::Const(2),
                    Instruction::Const(3),
                    Instruction::NewObject { count: 2 },
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 1,
                    },
                    Instruction::Return,
                ],
                0,
            ))
            .expect("symbol inspection probe emits");
            let wrapper = format!(
                r#"
static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != UINT16_C(1) || argc != 1 || arguments == NULL || result == NULL ||
        arguments[0].tag != KILN_VALUE_STRING || arguments[0].len != 8 ||
        memcmp(arguments[0].ptr, "{{ a: 2 }}", 8) != 0) return 1;
    *result = kiln_c99_undefined();
    return 0;
}}
int main(void) {{
    kiln_host host = {{host_call, NULL}};
    kiln_value result;
    return {symbol}(&host, &result) == KILN_C99_OK ? 0 : 2;
}}
"#,
                symbol = generated.symbol
            );
            let run = run_with_main(&generated, &wrapper, "symbol-inspection");
            assert!(
                run.status.success(),
                "symbol-only properties must not leave malformed inspection separators"
            );
        }
    }

    #[test]
    fn executable_array_string_iterator_and_named_enumeration_are_distinct() {
        let generated = emit(&program(
            vec![Constant::String("@@iterator".into())],
            vec![
                Instruction::NewArray { count: 0 },
                Instruction::Const(0),
                Instruction::GetComputed,
                Instruction::Return,
            ],
            0,
        ))
        .expect("array string iterator probe emits");
        let wrapper = format!(
            r#"
int main(void) {{
    kiln_value result;
    if ({symbol}(NULL, &result) != KILN_C99_OK) return 2;
    return result.tag == KILN_VALUE_UNDEFINED ? 0 : 1;
}}
"#,
            symbol = generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "array-string-iterator");
        assert!(
            run.status.success(),
            "an absent ordinary @@iterator key must not expose Symbol.iterator"
        );

        let constants = vec![
            Constant::Number(0.0f64.to_bits()),
            Constant::Number(1.0f64.to_bits()),
            Constant::Number(2.0f64.to_bits()),
            Constant::Number(3.0f64.to_bits()),
            Constant::String("a".into()),
            Constant::String("b".into()),
            Constant::String("keys".into()),
            Constant::String("values".into()),
            Constant::String("entries".into()),
        ];
        let array = |code: &mut Vec<Instruction>| {
            code.extend([
                Instruction::Const(1),
                Instruction::NewArray { count: 1 },
                Instruction::Dup,
                Instruction::Const(5),
                Instruction::Const(2),
                Instruction::SetComputed,
                Instruction::Pop,
                Instruction::Dup,
                Instruction::Const(4),
                Instruction::Const(3),
                Instruction::SetComputed,
                Instruction::Pop,
            ]);
        };
        let probes = [
            (6, 2, 4, "keys"),    // Object.keys(array)[2] == "a".
            (7, 2, 3, "values"),  // Object.values(array)[2] == 3.
            (8, 1, 5, "entries"), // Object.entries(array)[1][0] == "b".
        ];
        for (method, index, expected, label) in probes {
            let mut code = vec![Instruction::LoadBuiltin(kiln_ir::Builtin::Object)];
            array(&mut code);
            code.push(Instruction::CallMethod {
                name: method,
                argc: 1,
            });
            code.push(Instruction::Const(index));
            code.push(Instruction::GetComputed);
            if method == 8 {
                code.push(Instruction::Const(0));
                code.push(Instruction::GetComputed);
                code.push(Instruction::Const(5));
            } else {
                code.push(Instruction::Const(expected));
            }
            code.extend([
                Instruction::Binary(BinaryOp::StrictEqual),
                Instruction::Return,
            ]);
            let generated =
                emit(&program(constants.clone(), code, 0)).expect("array enumeration probe emits");
            let wrapper = format!(
                "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 1; }}\n",
                generated.symbol
            );
            let run = run_with_main(&generated, &wrapper, label);
            assert!(
                run.status.success(),
                "array {label} omitted named string properties"
            );
        }

        let mut code = Vec::new();
        array(&mut code);
        code.push(Instruction::EnumerateKeys);
        code.push(Instruction::Const(2));
        code.push(Instruction::GetComputed);
        code.push(Instruction::Const(4));
        code.extend([
            Instruction::Binary(BinaryOp::StrictEqual),
            Instruction::Return,
        ]);
        let generated = emit(&program(constants, code, 0)).expect("for-in probe emits");
        let wrapper = format!(
            "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 1; }}\n",
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "array-for-in");
        assert!(
            run.status.success(),
            "for-in omitted named string properties"
        );
    }

    #[test]
    fn executable_for_in_follows_prototypes_and_keeps_constructor_hidden() {
        let constants = vec![
            Constant::Number(1.0f64.to_bits()),
            Constant::String("prototype".into()),
            Constant::String("inherited".into()),
            Constant::Number(0.0f64.to_bits()),
            Constant::String("length".into()),
            Constant::Boolean(false),
        ];
        let generated = emit(&Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants,
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::StoreLocal(0),
                // Touching `prototype` creates its ordinary object and its non-enumerable
                // constructor link.
                Instruction::LoadLocal(0),
                Instruction::GetProperty(1),
                Instruction::StoreLocal(1),
                Instruction::LoadLocal(1),
                Instruction::Const(2),
                Instruction::Const(0),
                Instruction::SetComputed,
                Instruction::Pop,
                // `new C()` links the fresh instance to that prototype.
                Instruction::LoadLocal(0),
                Instruction::New { argc: 0 },
                Instruction::EnumerateKeys,
                Instruction::StoreLocal(2),
                Instruction::LoadLocal(2),
                Instruction::GetProperty(4),
                Instruction::Const(0),
                Instruction::Binary(BinaryOp::StrictEqual),
                Instruction::JumpIfFalse(25),
                Instruction::LoadLocal(2),
                Instruction::Const(3),
                Instruction::GetComputed,
                Instruction::Const(2),
                Instruction::Binary(BinaryOp::StrictEqual),
                Instruction::Return,
                Instruction::Const(5),
                Instruction::Return,
            ],
            local_count: 3,
            functions: vec![function(vec![Instruction::Return])],
        })
        .expect("prototype enumeration probe emits");
        let wrapper = format!(
            r#"
int main(void) {{
    kiln_value result;
    if ({symbol}(NULL, &result) != KILN_C99_OK) return 2;
    return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 3;
}}
"#,
            symbol = generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "prototype-enumeration");
        assert!(
            run.status.success(),
            "for-in must include enumerable prototype properties and omit constructor (exit {:?})",
            run.status.code()
        );
    }

    #[test]
    fn executable_array_named_property_is_visible_to_in() {
        let generated = emit(&program(
            vec![
                Constant::Number(1.0f64.to_bits()),
                Constant::String("total".into()),
                Constant::Number(9.0f64.to_bits()),
            ],
            vec![
                Instruction::Const(0),
                Instruction::NewArray { count: 1 },
                Instruction::StoreLocal(0),
                Instruction::LoadLocal(0),
                Instruction::Const(1),
                Instruction::Const(2),
                Instruction::SetComputed,
                Instruction::Pop,
                Instruction::Const(1),
                Instruction::LoadLocal(0),
                Instruction::Binary(BinaryOp::In),
                Instruction::Return,
            ],
            1,
        ))
        .expect("array named-property probe emits");
        let wrapper = format!(
            "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 3; }}\n",
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "array-named-in");
        assert!(
            run.status.success(),
            "the `in` operator must see an array's named property (exit {:?})",
            run.status.code()
        );
    }

    #[test]
    fn executable_typed_array_views_cover_all_element_kinds_and_aliasing() {
        let generated = emit(&program(vec![], vec![Instruction::Return], 0)).expect("emits");
        let wrapper = format!(
            r#"
int main(void) {{
    kiln_value buffer;
    kiln_value bytes;
    kiln_value words;
    kiln_value source;
    kiln_value sub;
    kiln_value iterator;
    kiln_value step;
    kiln_value keys;
    kiln_value prototype;
    kiln_value common;
    kiln_value value;
    kiln_value args[3];
    kiln_value thrown = kiln_c99_undefined();
    kiln_typed_kind kinds[] = {{KILN_TYPED_INT8, KILN_TYPED_UINT8, KILN_TYPED_UINT8_CLAMPED,
        KILN_TYPED_INT16, KILN_TYPED_UINT16, KILN_TYPED_INT32, KILN_TYPED_UINT32,
        KILN_TYPED_FLOAT32, KILN_TYPED_FLOAT64}};
    double inputs[] = {{200.0, 256.0, 300.0, 70000.0, 70000.0, 4294967297.0,
        -1.0, 1.1, 1.1}};
    double expected[] = {{-56.0, 0.0, 255.0, 4464.0, 4464.0, 1.0,
        4294967295.0, 1.1, 1.1}};
    size_t index;
    int found;
    int enumerable;
    if (kiln_c99_new_typed_buffer(&buffer, 8) != KILN_C99_OK) return 2;
    if (kiln_c99_new_typed_array(&bytes, KILN_TYPED_UINT8, buffer, 0, 0, 0, &thrown) != KILN_C99_OK) return 3;
    if (kiln_c99_new_typed_array(&words, KILN_TYPED_UINT32, buffer, 0, 0, 0, &thrown) != KILN_C99_OK) return 4;
    if (kiln_c99_typed_array_of(bytes)->length != 8 || kiln_c99_typed_array_of(words)->length != 2) return 5;
    kiln_c99_typed_set(kiln_c99_typed_array_of(bytes), 0, 1.0);
    kiln_c99_typed_set(kiln_c99_typed_array_of(bytes), 1, 2.0);
    kiln_c99_typed_set(kiln_c99_typed_array_of(bytes), 2, 3.0);
    kiln_c99_typed_set(kiln_c99_typed_array_of(bytes), 3, 4.0);
    if (kiln_c99_typed_get(kiln_c99_typed_array_of(words), 0) != 67305985.0) return 6;
    if (kiln_c99_new_typed_array(&source, KILN_TYPED_UINT8, buffer, 0, 0, 0, &thrown) != KILN_C99_OK) return 7;
    if (kiln_c99_invoke_method(NULL, &sub, KILN_C99_METHOD_TYPED_SUBARRAY, source,
                               (kiln_value[]){{kiln_c99_number(1.0), kiln_c99_number(3.0)}}, 2, 0,
                               &thrown) != KILN_C99_OK) return 8;
    kiln_c99_typed_set(kiln_c99_typed_array_of(sub), 0, 9.0);
    if (kiln_c99_typed_get(kiln_c99_typed_array_of(bytes), 1) != 9.0) return 9;
    for (index = 0; index < sizeof kinds / sizeof *kinds; ++index) {{
        if (kiln_c99_new_array(&source) != KILN_C99_OK ||
            kiln_c99_array_push(source, kiln_c99_number(inputs[index])) != KILN_C99_OK ||
            kiln_c99_new_typed_array(&value, kinds[index], source, 0, 0, 0, &thrown) != KILN_C99_OK) return 10;
        if (kinds[index] == KILN_TYPED_FLOAT32) {{
            if (fabs(kiln_c99_typed_get(kiln_c99_typed_array_of(value), 0) - expected[index]) > 0.000001) return 11;
        }} else if (kiln_c99_typed_get(kiln_c99_typed_array_of(value), 0) != expected[index]) return 12;
    }}
    if (kiln_c99_new_typed_iterator(&iterator, source, 0) != KILN_C99_OK ||
        kiln_c99_call(NULL, &step, kiln_c99_method(KILN_C99_METHOD_ITERATOR_NEXT), iterator,
                      NULL, 0, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_get(NULL, step, "value", 5, &value, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_as_number(value) != inputs[sizeof inputs / sizeof *inputs - 1]) return 13;
    /* ToIndex must reject before an out-of-range floating-to-size_t cast, while NaN is zero. */
    if (kiln_c99_to_index(kiln_c99_number(NAN), &index, &thrown) != KILN_C99_OK || index != 0)
        return 14;
    if (kiln_c99_to_index(kiln_c99_number(-1.0), &index, &thrown) != KILN_C99_THROWN ||
        kiln_c99_to_index(kiln_c99_number(1e300), &index, &thrown) != KILN_C99_THROWN)
        return 15;
    /* Extra offset/length arguments are ignored for an array source without being coerced. */
    if (kiln_c99_new_array(&source) != KILN_C99_OK ||
        kiln_c99_array_push(source, kiln_c99_number(7.0)) != KILN_C99_OK) return 16;
    args[0] = source;
    args[1] = kiln_c99_number(1e300);
    if (kiln_c99_construct(NULL, &value, kiln_c99_builtin(KILN_C99_BUILTIN_UINT8_ARRAY),
                           args, 2, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_typed_array_of(value)->length != 1 ||
        kiln_c99_typed_get(kiln_c99_typed_array_of(value), 0) != 7.0) return 17;
    args[0] = kiln_c99_number(-1.0);
    if (kiln_c99_construct(NULL, &value, kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER),
                           args, 1, 0, &thrown) != KILN_C99_THROWN) return 18;
    /* The view's indices are own and enumerable. Metadata and methods are inherited. */
    kiln_c99_typed_own(bytes, "0", 1, &found, &enumerable);
    if (!found || !enumerable) return 19;
    kiln_c99_typed_own(bytes, "length", 6, &found, &enumerable);
    if (found || enumerable) return 20;
    if (kiln_c99_own_keys(NULL, bytes, &keys, 0, NULL) != KILN_C99_OK ||
        kiln_c99_array_of(keys)->count != 8) return 21;
    if (kiln_c99_get(NULL, kiln_c99_builtin(KILN_C99_BUILTIN_UINT8_ARRAY), "prototype", 9,
                     &prototype, 0, &thrown) != KILN_C99_OK ||
        prototype.tag != KILN_VALUE_BUILTIN ||
        prototype.bits != KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE) return 22;
    if (kiln_c99_get(NULL, prototype, "constructor", 11, &value, 0, &thrown) != KILN_C99_OK ||
        value.tag != KILN_VALUE_BUILTIN || value.bits != KILN_C99_BUILTIN_UINT8_ARRAY) return 23;
    if (kiln_c99_get(NULL, prototype, "BYTES_PER_ELEMENT", 17, &value, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_as_number(value) != 1.0) return 24;
    if (kiln_c99_get(NULL, prototype, "set", 3, &value, 0, &thrown) != KILN_C99_OK ||
        value.tag != KILN_VALUE_METHOD || value.bits != KILN_C99_METHOD_TYPED_SET) return 25;
    args[0] = bytes;
    if (kiln_c99_invoke_method(NULL, &value, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                               kiln_c99_undefined(), args, 1, 0, &thrown) != KILN_C99_OK ||
        value.tag != prototype.tag || value.bits != prototype.bits) return 26;
    args[0] = prototype;
    if (kiln_c99_invoke_method(NULL, &common, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                               kiln_c99_undefined(), args, 1, 0, &thrown) != KILN_C99_OK ||
        common.tag != KILN_VALUE_BUILTIN ||
        common.bits != KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE) return 27;
    if (kiln_c99_get(NULL, kiln_c99_builtin(KILN_C99_BUILTIN_SYMBOL), "toStringTag", 11,
                     &value, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_get_computed(NULL, bytes, value, &step, 0, &thrown) != KILN_C99_OK ||
        step.tag != KILN_VALUE_STRING || step.len != 10 ||
        memcmp(step.ptr, "Uint8Array", 10) != 0) return 28;
    return 0;
}}
/* {} */
"#,
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "typed-array-core");
        assert!(
            run.status.success(),
            "typed-array C core failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_inspection_skips_symbol_properties_without_separators() {
        let generated = emit(&program(
            vec![
                Constant::String("iterator".into()),
                Constant::Number(1.0f64.to_bits()),
                Constant::String("a".into()),
                Constant::Number(2.0f64.to_bits()),
            ],
            vec![
                Instruction::LoadBuiltin(kiln_ir::Builtin::Symbol),
                Instruction::GetProperty(0),
                Instruction::Const(1),
                Instruction::Const(2),
                Instruction::Const(3),
                Instruction::NewObject { count: 2 },
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Return,
            ],
            0,
        ))
        .expect("symbol inspection probe emits");
        let wrapper = format!(
            r#"
static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != UINT16_C(1) || argc != 1 || arguments == NULL || result == NULL ||
        arguments[0].tag != KILN_VALUE_STRING || arguments[0].len != 8 ||
        memcmp(arguments[0].ptr, "{{ a: 2 }}", 8) != 0) return 1;
    *result = kiln_c99_undefined();
    return 0;
}}
int main(void) {{
    kiln_host host = {{host_call, NULL}};
    kiln_value result;
    return {symbol}(&host, &result) == KILN_C99_OK ? 0 : 2;
}}
"#,
            symbol = generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "symbol-inspection");
        assert!(
            run.status.success(),
            "symbol-only properties must not leave malformed inspection separators"
        );
    }

    #[test]
    fn executable_url_constructs_path_and_decodes_query_values() {
        let generated = emit(&program(
            vec![
                Constant::String("/echo?q=one%20two".into()),
                Constant::String("http://127.0.0.1".into()),
                Constant::String("pathname".into()),
                Constant::String("/echo".into()),
                Constant::String("searchParams".into()),
                Constant::String("get".into()),
                Constant::String("q".into()),
                Constant::String("one two".into()),
                Constant::Boolean(false),
            ],
            vec![
                Instruction::LoadBuiltin(kiln_ir::Builtin::Url),
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::New { argc: 2 },
                Instruction::StoreLocal(0),
                Instruction::LoadLocal(0),
                Instruction::GetProperty(2),
                Instruction::Const(3),
                Instruction::Binary(BinaryOp::StrictEqual),
                Instruction::JumpIfFalse(20),
                Instruction::LoadLocal(0),
                Instruction::GetProperty(4),
                Instruction::Const(6),
                Instruction::CallMethod { name: 5, argc: 1 },
                Instruction::Const(7),
                Instruction::Binary(BinaryOp::StrictEqual),
                Instruction::Return,
                Instruction::Const(8),
                Instruction::Return,
                Instruction::Const(8),
                Instruction::Return,
            ],
            1,
        ))
        .expect("URL probe emits");
        let wrapper = format!(
            "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 3; }}\n",
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "url-routing");
        assert!(
            run.status.success(),
            "native URL path/query behavior failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_buffer_reuses_uint8_storage_with_node_semantics() {
        let generated = emit(&program(
            vec![
                Constant::String("kiln ".into()),
                Constant::String("streams".into()),
                Constant::String("from".into()),
                Constant::String("concat".into()),
                Constant::String("toString".into()),
                Constant::String("kiln streams".into()),
                Constant::String("isBuffer".into()),
                Constant::Boolean(false),
            ],
            vec![
                Instruction::LoadBuiltin(kiln_ir::Builtin::Buffer),
                Instruction::StoreLocal(0),
                Instruction::LoadLocal(0),
                Instruction::Const(0),
                Instruction::CallMethod { name: 2, argc: 1 },
                Instruction::LoadLocal(0),
                Instruction::Const(1),
                Instruction::CallMethod { name: 2, argc: 1 },
                Instruction::NewArray { count: 2 },
                Instruction::StoreLocal(2),
                Instruction::LoadLocal(0),
                Instruction::LoadLocal(2),
                Instruction::CallMethod { name: 3, argc: 1 },
                Instruction::StoreLocal(1),
                Instruction::LoadLocal(1),
                Instruction::CallMethod { name: 4, argc: 0 },
                Instruction::Const(5),
                Instruction::Binary(BinaryOp::StrictEqual),
                Instruction::JumpIfFalse(26),
                Instruction::LoadLocal(0),
                Instruction::LoadLocal(1),
                Instruction::CallMethod { name: 6, argc: 1 },
                Instruction::Return,
                Instruction::Const(7),
                Instruction::Return,
                Instruction::Const(7),
                Instruction::Return,
                Instruction::Const(7),
                Instruction::Return,
            ],
            3,
        ))
        .expect("Buffer probe emits");
        let wrapper = format!(
            "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 3; }}\n",
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "buffer-facade");
        assert!(
            run.status.success(),
            "native Buffer facade failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn executable_node_support_facades_are_deterministic_and_callback_driven() {
        let generated = emit(&program(
            vec![Constant::Undefined],
            vec![Instruction::Const(0), Instruction::Return],
            0,
        ))
        .expect("support facade probe emits");
        let wrapper = r#"
static int callback(kiln_host *host, kiln_value *result, kiln_value *upvalues,
                    kiln_value receiver, const kiln_value *args, size_t argc, int depth,
                    kiln_value *thrown, kiln_generator *frame) {
    (void)host; (void)upvalues; (void)receiver; (void)depth; (void)thrown; (void)frame;
    if (argc == 2 && args[0].tag == KILN_VALUE_NULL && args[1].tag == KILN_VALUE_ARRAY &&
        kiln_c99_array_of(args[1])->count == 2) {
        *result = kiln_c99_boolean(1);
        return KILN_C99_OK;
    }
    if (argc == 1 && args[0].tag == KILN_VALUE_STRING && args[0].len == 7 &&
        memcmp(args[0].ptr, "forward", 7) == 0) {
        *result = kiln_c99_boolean(1);
        return KILN_C99_OK;
    }
    *result = kiln_c99_boolean(0);
    return KILN_C99_OK;
}
int main(void) {
    uint64_t realm;
    kiln_value thrown = kiln_c99_undefined();
    kiln_value result;
    kiln_value args[4];
    kiln_value callable;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_make_closure(&callable, callback, "callback", 8, NULL, 0, "<test>", 0, 0, 0, 0, 0, 0,
                              1, kiln_c99_undefined(), kiln_c99_undefined(), 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK)
        return 1;
    args[0] = kiln_c99_copy_text("%s:%d", 5);
    args[1] = kiln_c99_copy_text("port", 4);
    args[2] = kiln_c99_number(7.0);
    if (kiln_c99_node_util(NULL, &result, KILN_C99_METHOD_NODE_UTIL_FORMAT,
                           args, 3, 0, &thrown) != KILN_C99_OK ||
        result.tag != KILN_VALUE_STRING || result.len != 6 ||
        memcmp(result.ptr, "port:7", 6) != 0) return 2;
    args[0] = kiln_c99_undefined();
    args[1] = callable;
    args[2] = kiln_c99_undefined();
    args[3] = kiln_c99_copy_text("forward", 7);
    if (kiln_c99_node_diagnostics(NULL, &result,
            KILN_C99_METHOD_NODE_DIAGNOSTICS_RUN_STORES, args, 4, 0, &thrown) != KILN_C99_OK ||
        result.tag != KILN_VALUE_BOOLEAN || !result.bits) return 3;
    args[0] = kiln_c99_copy_text("localhost", 9);
    if (kiln_c99_new_object(&args[1]) != KILN_C99_OK ||
        kiln_c99_put(NULL, args[1], "all", 3, kiln_c99_boolean(1), 0, NULL) != KILN_C99_OK)
        return 4;
    args[2] = callable;
    if (kiln_c99_node_dns_lookup(NULL, &result, args, 3, 0, &thrown) != KILN_C99_OK)
        return 5;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 6;
}
"#;
        let run = run_with_main(&generated, wrapper, "node-support-facades");
        assert!(
            run.status.success(),
            "native Node support facades failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }
}
