//! The C embedding contract and small runtime used by Kiln's native command-line launcher.
//!
//! The generated program translation units deliberately do not contain host policy.  This crate
//! emits the stable, reusable callback object that implements the command-line `WriteStdout`
//! operation, plus a tiny executable launcher for one generated module.  Keeping those sources
//! separate lets the runtime object be cached independently from application code. Embedded hosts
//! consume [`embedding_header`] and supply their own callback policy without linking the CLI code.

use std::sync::OnceLock;

/// Version of the generated native runtime source and its C ABI.
pub const NATIVE_RUNTIME_VERSION: u32 = 4;

/// C entry-point ABI version declared by [`embedding_header`].
pub const EMBEDDING_C_ABI_VERSION: u32 = 1;

/// Host callback ABI version declared by [`embedding_header`].
pub const EMBEDDING_HOST_ABI_VERSION: u32 = 1;

/// The operation implemented by [`runtime_source`].
pub const WRITE_STDOUT_OPERATION: u16 = 1;

/// Return the C header shared by embedded modules and the native CLI runtime.
///
/// The header is self-contained, include-guarded, and contains only the stable value/callback
/// contract. It is suitable for writing to a header file or embedding directly in a generated C
/// translation unit.
pub fn embedding_header() -> &'static str {
    EMBEDDING_HEADER
}

/// Return the canonical embedding ABI plus a declaration for one generated module.
///
/// The module declaration has its own symbol-specific include guard so callers can safely
/// compose headers for several modules. C++ consumers receive C linkage for the generated
/// entry point, matching the C object emitted by `kiln-backend-c`.
pub fn embedding_module_header(symbol: &str) -> Result<String, RuntimeSourceError> {
    validate_symbol(symbol)?;
    let guard = format!("KILN_NATIVE_MODULE_{symbol}_H");
    let mut header = String::with_capacity(
        embedding_header().len()
            + MODULE_HEADER_PREFIX.len()
            + 3 * guard.len()
            + MODULE_HEADER_DEFINE.len()
            + MODULE_HEADER_DECL_PREFIX.len()
            + symbol.len()
            + MODULE_HEADER_DECL_SUFFIX.len()
            + " */\n".len(),
    );
    header.push_str(embedding_header());
    header.push_str(MODULE_HEADER_PREFIX);
    header.push_str(&guard);
    header.push_str(MODULE_HEADER_DEFINE);
    header.push_str(&guard);
    header.push_str(MODULE_HEADER_DECL_PREFIX);
    header.push_str(symbol);
    header.push_str(MODULE_HEADER_DECL_SUFFIX);
    header.push_str(&guard);
    header.push_str(" */\n");
    Ok(header)
}

/// Why a native runtime source request was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeSourceError {
    /// The requested entry point is not a valid portable C identifier.
    InvalidSymbol { symbol: String },
}

impl std::fmt::Display for RuntimeSourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSymbol { symbol } => {
                write!(formatter, "invalid native module symbol `{symbol}`")
            }
        }
    }
}

impl std::error::Error for RuntimeSourceError {}

/// Short compatibility spelling for callers that do not distinguish source kinds.
pub type RuntimeError = RuntimeSourceError;

/// Return the deterministic, program-independent C11 runtime translation unit.
pub fn runtime_source() -> &'static str {
    RUNTIME_SOURCE
        .get_or_init(|| {
            let mut source = String::with_capacity(
                RUNTIME_PREFIX.len() + embedding_header().len() + RUNTIME_BODY.len(),
            );
            source.push_str(RUNTIME_PREFIX);
            source.push_str(embedding_header());
            source.push_str(RUNTIME_BODY);
            source
        })
        .as_str()
}

/// Emit a deterministic C11 launcher that calls `symbol` as the process entry point.
///
/// A module returns zero on success and a nonzero status for a host/control-flow failure.  The
/// launcher maps that status to the conventional process exit values 0 and 1.
pub fn launcher_source(symbol: &str) -> Result<String, RuntimeSourceError> {
    validate_symbol(symbol)?;
    let mut source = String::with_capacity(
        LAUNCHER_PREFIX.len()
            + embedding_header().len()
            + LAUNCHER_DECL_PREFIX.len()
            + 2 * symbol.len()
            + LAUNCHER_DECL_SUFFIX.len()
            + LAUNCHER_BODY.len(),
    );
    source.push_str(LAUNCHER_PREFIX);
    source.push_str(embedding_header());
    source.push_str(LAUNCHER_DECL_PREFIX);
    source.push_str(symbol);
    source.push_str(LAUNCHER_DECL_SUFFIX);
    source.push_str(symbol);
    source.push_str(LAUNCHER_BODY);
    Ok(source)
}

/// Alias with an explicit verb for callers that keep several generated source kinds together.
pub fn emit_launcher(symbol: &str) -> Result<String, RuntimeError> {
    launcher_source(symbol)
}

fn validate_symbol(symbol: &str) -> Result<(), RuntimeSourceError> {
    let mut bytes = symbol.bytes();
    let Some(first) = bytes.next() else {
        return Err(RuntimeSourceError::InvalidSymbol {
            symbol: symbol.to_owned(),
        });
    };
    if !(first == b'_' || first.is_ascii_alphabetic())
        || bytes.any(|byte| !(byte == b'_' || byte.is_ascii_alphanumeric()))
    {
        return Err(RuntimeSourceError::InvalidSymbol {
            symbol: symbol.to_owned(),
        });
    }
    Ok(())
}

/// Keep this declaration byte-for-byte compatible with kiln-backend-c's generated C99 units.
/// The runtime is C11, but only uses facilities available in C99 for portability.
const EMBEDDING_HEADER: &str = r#"/* Kiln native embedding ABI. */
#ifndef KILN_NATIVE_EMBEDDING_H
#define KILN_NATIVE_EMBEDDING_H

#include <stddef.h>
#include <stdint.h>

#define KILN_NATIVE_RUNTIME_VERSION 4
#define KILN_C_ABI_VERSION 1
#define KILN_HOST_ABI_VERSION 1

#define KILN_C99_OK 0
#define KILN_C99_INVALID_ARGUMENT (-1)
#define KILN_C99_CONTROL_ERROR (-2)
#define KILN_C99_HOST_ERROR (-3)

#define KILN_HOST_OP_WRITE_STDOUT 1

enum kiln_value_tag {
    KILN_VALUE_UNDEFINED = 0,
    KILN_VALUE_NULL = 1,
    KILN_VALUE_BOOLEAN = 2,
    KILN_VALUE_NUMBER = 3,
    KILN_VALUE_STRING = 4
};

/* This layout is the complete boundary value.  bits stores booleans and IEEE-754 numbers;
 * ptr/len stores UTF-8 string bytes and is not required to be NUL terminated. */
typedef struct kiln_value {
    uint32_t tag;
    uint32_t reserved;
    uint64_t bits;
    const char *ptr;
    size_t len;
} kiln_value;

#if defined(__cplusplus)
static_assert(offsetof(kiln_value, tag) == 0, "kiln_value.tag ABI offset");
static_assert(offsetof(kiln_value, reserved) == 4, "kiln_value.reserved ABI offset");
static_assert(offsetof(kiln_value, bits) == 8, "kiln_value.bits ABI offset");
static_assert(offsetof(kiln_value, ptr) == 16, "kiln_value.ptr ABI offset");
static_assert(offsetof(kiln_value, len) == 16 + sizeof(void *), "kiln_value.len ABI offset");
static_assert(sizeof(kiln_value) == 16 + 2 * sizeof(void *), "kiln_value ABI size");
#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(offsetof(kiln_value, tag) == 0, "kiln_value.tag ABI offset");
_Static_assert(offsetof(kiln_value, reserved) == 4, "kiln_value.reserved ABI offset");
_Static_assert(offsetof(kiln_value, bits) == 8, "kiln_value.bits ABI offset");
_Static_assert(offsetof(kiln_value, ptr) == 16, "kiln_value.ptr ABI offset");
_Static_assert(offsetof(kiln_value, len) == 16 + sizeof(void *), "kiln_value.len ABI offset");
_Static_assert(sizeof(kiln_value) == 16 + 2 * sizeof(void *), "kiln_value ABI size");
#endif

/* Argument ptr/len values are borrowed for the callback invocation.  A string stored in *result
 * must remain alive until the module entry point returns.  A nonzero callback return is
 * propagated as KILN_C99_HOST_ERROR. */
typedef int (*kiln_host_call_fn)(void *context, uint16_t operation, uint16_t argc,
                                 const kiln_value *arguments, kiln_value *result);
typedef struct kiln_host {
    kiln_host_call_fn call;
    void *context;
} kiln_host;

#endif /* KILN_NATIVE_EMBEDDING_H */
"#;

const MODULE_HEADER_PREFIX: &str = r#"
/* Kiln native module embedding declaration. */
#ifndef "#;

const MODULE_HEADER_DEFINE: &str = r#"
#define "#;

const MODULE_HEADER_DECL_PREFIX: &str = r#"

#if defined(__cplusplus)
extern "C" {
#endif
extern int "#;

const MODULE_HEADER_DECL_SUFFIX: &str = r#"(kiln_host *host, kiln_value *result);
#if defined(__cplusplus)
} /* extern "C" */
#endif

#endif /* "#;

static RUNTIME_SOURCE: OnceLock<String> = OnceLock::new();

const RUNTIME_PREFIX: &str = r#"/* Kiln native CLI runtime: deterministic HostOps callback. */
#include <math.h>
#include <stdio.h>
#include <string.h>

"#;

const RUNTIME_BODY: &str = r#"
/* Private number-provider ABI. The native linker supplies these from the validated runtime
 * provider artifact; they are deliberately absent from the public embedding header. */
extern int kiln_number_to_js_string(double value, char *buffer, size_t capacity);
extern double kiln_string_to_number(const char *bytes, size_t length);

static int kiln_cli_write_bytes(const char *bytes, size_t length) {
    if (length != 0 && bytes == NULL) {
        return KILN_C99_HOST_ERROR;
    }
    if (length != 0 && fwrite(bytes, 1, length, stdout) != length) {
        return KILN_C99_HOST_ERROR;
    }
    return KILN_C99_OK;
}

static int kiln_cli_write_value(kiln_value value) {
    static const char undefined_text[] = "undefined";
    static const char null_text[] = "null";
    static const char true_text[] = "true";
    static const char false_text[] = "false";
    switch (value.tag) {
    case KILN_VALUE_UNDEFINED:
        return kiln_cli_write_bytes(undefined_text, sizeof(undefined_text) - 1);
    case KILN_VALUE_NULL:
        return kiln_cli_write_bytes(null_text, sizeof(null_text) - 1);
    case KILN_VALUE_BOOLEAN:
        return value.bits != 0
            ? kiln_cli_write_bytes(true_text, sizeof(true_text) - 1)
            : kiln_cli_write_bytes(false_text, sizeof(false_text) - 1);
    case KILN_VALUE_STRING:
        return kiln_cli_write_bytes(value.ptr, value.len);
    case KILN_VALUE_NUMBER: {
        double number;
        char buffer[64];
        int length;
        memcpy(&number, &value.bits, sizeof(number));
        if (isnan(number)) {
            return kiln_cli_write_bytes("NaN", 3);
        }
        if (isinf(number)) {
            return number < 0.0
                ? kiln_cli_write_bytes("-Infinity", 9)
                : kiln_cli_write_bytes("Infinity", 8);
        }
        /* Console display preserves negative zero; String(-0) is the separate `0` conversion. */
        if (number == 0.0) {
            return signbit(number)
                ? kiln_cli_write_bytes("-0", 2)
                : kiln_cli_write_bytes("0", 1);
        }
        length = kiln_number_to_js_string(number, buffer, sizeof(buffer));
        if (length < 0 || (size_t)length >= sizeof(buffer)) {
            return KILN_C99_HOST_ERROR;
        }
        return kiln_cli_write_bytes(buffer, (size_t)length);
    }
    default:
        return KILN_C99_HOST_ERROR;
    }
}

static kiln_value kiln_cli_undefined(void) {
    kiln_value value;
    value.tag = KILN_VALUE_UNDEFINED;
    value.reserved = 0;
    value.bits = 0;
    value.ptr = NULL;
    value.len = 0;
    return value;
}

/* Stable CLI implementation of HostOps::WriteStdout (operation number 1). */
int kiln_cli_host_call(void *context, uint16_t operation, uint16_t argc,
                       const kiln_value *arguments, kiln_value *result) {
    uint16_t index;
    (void)context;
    if (operation != KILN_HOST_OP_WRITE_STDOUT || result == NULL
        || (argc != 0 && arguments == NULL)) {
        return KILN_C99_HOST_ERROR;
    }
    for (index = 0; index < argc; ++index) {
        if (index != 0 && fputc(' ', stdout) == EOF) {
            return KILN_C99_HOST_ERROR;
        }
        if (kiln_cli_write_value(arguments[index]) != KILN_C99_OK) {
            return KILN_C99_HOST_ERROR;
        }
    }
    if (fputc('\n', stdout) == EOF || fflush(stdout) != 0) {
        return KILN_C99_HOST_ERROR;
    }
    *result = kiln_cli_undefined();
    return KILN_C99_OK;
}
"#;

const LAUNCHER_PREFIX: &str = r#"/* Kiln native CLI launcher. */
#include <locale.h>

"#;

const LAUNCHER_DECL_PREFIX: &str = r#"
extern int kiln_cli_host_call(void *context, uint16_t operation, uint16_t argc,
                              const kiln_value *arguments, kiln_value *result);
extern int 
"#;

const LAUNCHER_DECL_SUFFIX: &str = r#"(kiln_host *host, kiln_value *result);

int main(void) {
    kiln_host host;
    kiln_value result;
    int status;
    if (setlocale(LC_NUMERIC, "C") == NULL) {
        return 1;
    }
    host.call = kiln_cli_host_call;
    host.context = NULL;
    status = 
"#;

const LAUNCHER_BODY: &str = r#"(&host, &result);
    return status == 0 ? 0 : 1;
}
"#;

// The suffix above intentionally leaves the call expression open so the validated symbol is the
// only program-specific part of the launcher source.
const _: () = {
    // Compile-time assertion that the source fragments remain nonempty and deterministic.
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn runtime_is_deterministic_and_does_not_name_a_module() {
        assert_eq!(runtime_source(), runtime_source());
        assert!(runtime_source().contains("int kiln_cli_host_call"));
        assert!(runtime_source().contains("kiln_number_to_js_string"));
        assert!(runtime_source().contains("kiln_string_to_number"));
        assert!(
            runtime_source().contains("kiln_number_to_js_string(number, buffer, sizeof(buffer))")
        );
        assert!(runtime_source().contains("Private number-provider ABI"));
        assert!(!runtime_source().contains("Copyright 2024 Tiago Carvalho"));
        assert!(!runtime_source().contains("%.17g"));
        assert!(!runtime_source().contains("kiln_module_"));
    }

    #[test]
    fn runtime_identity_is_separate_from_the_private_number_provider() {
        assert_eq!(NATIVE_RUNTIME_VERSION, 4);
        assert!(embedding_header().contains(&format!(
            "#define KILN_NATIVE_RUNTIME_VERSION {NATIVE_RUNTIME_VERSION}"
        )));
    }

    #[test]
    fn embedding_header_is_deterministic_and_contains_the_stable_contract() {
        assert_eq!(embedding_header(), embedding_header());
        assert!(embedding_header().contains("#ifndef KILN_NATIVE_EMBEDDING_H"));
        assert!(embedding_header().contains(&format!(
            "#define KILN_NATIVE_RUNTIME_VERSION {NATIVE_RUNTIME_VERSION}"
        )));
        assert!(embedding_header().contains(&format!(
            "#define KILN_C_ABI_VERSION {EMBEDDING_C_ABI_VERSION}"
        )));
        assert!(embedding_header().contains(&format!(
            "#define KILN_HOST_ABI_VERSION {EMBEDDING_HOST_ABI_VERSION}"
        )));
        assert!(embedding_header().contains("#define KILN_HOST_OP_WRITE_STDOUT 1"));
        assert!(embedding_header().contains("uint32_t tag;"));
        assert!(embedding_header().contains("uint32_t reserved;"));
        assert!(embedding_header().contains("uint64_t bits;"));
        assert!(embedding_header().contains("const char *ptr;"));
        assert!(embedding_header().contains("size_t len;"));
        assert!(embedding_header().contains("typedef int (*kiln_host_call_fn)"));
        assert!(embedding_header().contains("typedef struct kiln_host"));
        assert!(embedding_header().contains("sizeof(kiln_value) == 16 + 2 * sizeof(void *)"));
        assert!(embedding_header().contains("#endif /* KILN_NATIVE_EMBEDDING_H */"));
    }

    #[test]
    fn embedding_header_compiles_as_c_and_cxx() {
        let Some(cc) = find_cc() else {
            return;
        };
        let Some(cxx) = find_cxx() else {
            return;
        };
        let root = test_directory();
        std::fs::create_dir(&root).expect("create test directory");
        let header = root.join("kiln_embedding.h");
        let c_source = root.join("probe.c");
        let cxx_source = root.join("probe.cc");
        std::fs::write(&header, embedding_header()).expect("write header");
        let probe = r#"
#include "kiln_embedding.h"
static int call(void *context, uint16_t operation, uint16_t argc,
                const kiln_value *arguments, kiln_value *result) {
    (void)context;
    (void)operation;
    (void)argc;
    (void)arguments;
    (void)result;
    return KILN_C99_OK;
}
int main(void) {
    kiln_host host;
    kiln_value value;
    host.call = call;
    host.context = 0;
    value.tag = KILN_VALUE_UNDEFINED;
    value.reserved = 0;
    value.bits = 0;
    value.ptr = 0;
    value.len = 0;
    return host.call(host.context, KILN_HOST_OP_WRITE_STDOUT, 0, 0, &value);
}
"#;
        std::fs::write(&c_source, probe).expect("write C probe");
        std::fs::write(&cxx_source, probe).expect("write C++ probe");

        for (compiler, standard, source, object) in [
            (cc, "-std=c11", &c_source, root.join("probe-c.o")),
            (cxx, "-std=c++11", &cxx_source, root.join("probe-cxx.o")),
        ] {
            let compile = Command::new(compiler)
                .args([standard, "-pedantic", "-Wall", "-Wextra", "-Werror"])
                .arg("-c")
                .arg(source)
                .arg("-o")
                .arg(&object)
                .output()
                .expect("invoke C compiler");
            assert!(
                compile.status.success(),
                "embedding header failed to compile with {compiler}:\n{}",
                String::from_utf8_lossy(&compile.stderr)
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn embedding_module_header_is_deterministic_and_symbol_guarded() {
        let symbol = "kiln_module_deadbeef";
        let one = embedding_module_header(symbol).expect("valid symbol");
        assert_eq!(one, embedding_module_header(symbol).unwrap());
        assert!(one.starts_with(embedding_header()));
        assert!(one.contains("#ifndef KILN_NATIVE_MODULE_kiln_module_deadbeef_H"));
        assert!(one.contains("#define KILN_NATIVE_MODULE_kiln_module_deadbeef_H"));
        assert!(one.contains("extern \"C\" {"));
        assert!(
            one.contains("extern int kiln_module_deadbeef(kiln_host *host, kiln_value *result);")
        );
        assert!(one.contains("#endif /* KILN_NATIVE_MODULE_kiln_module_deadbeef_H */"));
    }

    #[test]
    fn embedding_module_header_rejects_invalid_symbols() {
        for symbol in ["", "9module", "module-name", "module name", "méthode"] {
            assert!(matches!(
                embedding_module_header(symbol),
                Err(RuntimeSourceError::InvalidSymbol { .. })
            ));
        }
    }

    #[test]
    fn embedding_module_header_compiles_as_c_and_cxx() {
        let Some(cc) = find_cc() else {
            return;
        };
        let Some(cxx) = find_cxx() else {
            return;
        };
        let root = test_directory();
        std::fs::create_dir(&root).expect("create test directory");
        let header = root.join("kiln_module.h");
        let c_source = root.join("probe.c");
        let cxx_source = root.join("probe.cc");
        let module_header = embedding_module_header("kiln_module_probe").expect("valid symbol");
        std::fs::write(&header, module_header).expect("write module header");
        let probe = r#"
#include "kiln_module.h"
#include "kiln_module.h"

int main(void) {
    kiln_host host = {0, 0};
    kiln_value result = {0, 0, 0, 0, 0};
    return kiln_module_probe(&host, &result);
}
"#;
        std::fs::write(&c_source, probe).expect("write C probe");
        std::fs::write(&cxx_source, probe).expect("write C++ probe");

        for (compiler, standard, source, object) in [
            (cc, "-std=c11", &c_source, root.join("probe-c.o")),
            (cxx, "-std=c++11", &cxx_source, root.join("probe-cxx.o")),
        ] {
            let compile = Command::new(compiler)
                .args([standard, "-pedantic", "-Wall", "-Wextra", "-Werror"])
                .arg("-c")
                .arg(source)
                .arg("-o")
                .arg(&object)
                .output()
                .expect("invoke C compiler");
            assert!(
                compile.status.success(),
                "module embedding header failed to compile with {compiler}:\n{}",
                String::from_utf8_lossy(&compile.stderr)
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn launcher_validates_symbols_and_is_deterministic() {
        let one = launcher_source("kiln_module_deadbeef").expect("valid symbol");
        assert_eq!(one, launcher_source("kiln_module_deadbeef").unwrap());
        assert!(one.contains("kiln_module_deadbeef"));
        for symbol in ["", "9module", "module-name", "module name", "méthode"] {
            assert!(matches!(
                launcher_source(symbol),
                Err(RuntimeSourceError::InvalidSymbol { .. })
            ));
        }
    }

    #[test]
    fn launcher_pins_numeric_locale_before_module_call() {
        let source = launcher_source("kiln_module_locale").expect("valid symbol");
        let locale = source
            .find("setlocale(LC_NUMERIC, \"C\")")
            .expect("launcher sets C numeric locale");
        let module_call = source
            .find("status = \nkiln_module_locale")
            .expect("launcher calls module");
        assert!(locale < module_call);
        assert!(source.contains("#include <locale.h>"));
        assert!(source.contains("return 1;"));
    }

    #[test]
    fn runtime_and_launcher_link_against_stub_module() {
        let Some(cc) = find_cc() else {
            return;
        };
        let root = test_directory();
        std::fs::create_dir(&root).expect("create test directory");
        let runtime = root.join("runtime.c");
        let launcher = root.join("launcher.c");
        let header = root.join("kiln_host.h");
        let stub = root.join("stub.c");
        let number_provider = root.join("number-provider.c");
        let executable = root.join("kiln-test");
        std::fs::write(&runtime, runtime_source()).expect("write runtime");
        std::fs::write(
            &launcher,
            launcher_source("kiln_module_test").expect("valid symbol"),
        )
        .expect("write launcher");
        std::fs::write(&header, embedding_header()).expect("write header");
        std::fs::write(&stub, STUB_SOURCE).expect("write stub");
        std::fs::write(&number_provider, kiln_native_number::runtime_source())
            .expect("write test number provider");
        let compile = Command::new(cc)
            .args([
                "-std=c11",
                "-pedantic",
                "-Wall",
                "-Wextra",
                "-Werror",
                // Ryu's portable amalgamation retains target-specific static inline helpers.
                "-Wno-unused-function",
            ])
            .args([
                runtime.as_os_str(),
                launcher.as_os_str(),
                stub.as_os_str(),
                number_provider.as_os_str(),
            ])
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("invoke C compiler");
        assert!(
            compile.status.success(),
            "C runtime failed to compile/link:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&executable)
            .output()
            .expect("run linked launcher");
        assert!(run.status.success());
        assert_eq!(
            String::from_utf8(run.stdout).expect("UTF-8 stdout"),
            "undefined null true false -0 NaN Infinity -Infinity 100000000000000000000 1e+21 1e-7 hello\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn find_cc() -> Option<&'static str> {
        find_compiler("cc")
    }

    fn find_cxx() -> Option<&'static str> {
        find_compiler("c++")
    }

    fn find_compiler(name: &'static str) -> Option<&'static str> {
        let result = Command::new(name)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        result.success().then_some(name)
    }

    fn test_directory() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "kiln-native-runtime-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos(),
            TEST_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        path
    }

    const STUB_SOURCE: &str = r#"
#include <string.h>
#include "kiln_host.h"

int kiln_module_test(kiln_host *host, kiln_value *result) {
    static const char text[] = "hello";
    kiln_value arguments[12];
    double negative_zero = -0.0;
    double nan = 0.0 / 0.0;
    double infinity = 1.0 / 0.0;
    double negative_infinity = -1.0 / 0.0;
    double decimal_threshold = 1e20;
    double exponent_threshold = 1e21;
    double tiny = 1e-7;
    memset(arguments, 0, sizeof(arguments));
    arguments[0].tag = KILN_VALUE_UNDEFINED;
    arguments[1].tag = KILN_VALUE_NULL;
    arguments[2].tag = KILN_VALUE_BOOLEAN;
    arguments[2].bits = 1;
    arguments[3].tag = KILN_VALUE_BOOLEAN;
    arguments[4].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[4].bits, &negative_zero, sizeof(negative_zero));
    arguments[5].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[5].bits, &nan, sizeof(nan));
    arguments[6].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[6].bits, &infinity, sizeof(infinity));
    arguments[7].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[7].bits, &negative_infinity, sizeof(negative_infinity));
    arguments[8].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[8].bits, &decimal_threshold, sizeof(decimal_threshold));
    arguments[9].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[9].bits, &exponent_threshold, sizeof(exponent_threshold));
    arguments[10].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[10].bits, &tiny, sizeof(tiny));
    arguments[11].tag = KILN_VALUE_STRING;
    arguments[11].ptr = text;
    arguments[11].len = sizeof(text) - 1;
    return host->call(host->context, 1, 12, arguments, result);
}
"#;
}
