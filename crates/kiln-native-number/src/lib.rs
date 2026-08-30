//! ECMAScript number conversion for the native C backend.
//!
//! [`source`] supplies binary64 formatting, [`parser_source`] supplies `StringNumericLiteral`
//! parsing, and [`runtime_source`] composes both into one deterministic C11 translation unit.

/// Version of the vendored formatter source and its C ABI.
pub const NUMBER_FORMAT_VERSION: u32 = 1;

/// Version of the string-to-number parser source and its internal C ABI.
pub const NUMBER_PARSE_VERSION: u32 = 1;

/// Return the deterministic C11 formatter source.
pub fn source() -> &'static str {
    include_str!("../c/kiln_number.c")
}

/// Return the self-contained C11 `StringNumericLiteral` parser source.
pub fn parser_source() -> &'static str {
    static PARSER_SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PARSER_SOURCE
        .get_or_init(|| {
            const INCLUDE: &str = "#include \"ffc.h\"";
            let adapter = include_str!("../c/kiln_string_number.c");
            let (before, after) = adapter
                .split_once(INCLUDE)
                .expect("the parser adapter includes the pinned ffc header exactly once");
            let ffc = include_str!("../c/ffc.h");
            let mut result = String::with_capacity(before.len() + ffc.len() + after.len() + 1);
            result.push_str(before);
            result.push_str(ffc);
            result.push_str(after);
            result
        })
        .as_str()
}

/// Return the complete internal native number runtime as one C11 translation unit.
pub fn runtime_source() -> &'static str {
    static RUNTIME_SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RUNTIME_SOURCE
        .get_or_init(|| {
            let parser = parser_source();
            let formatter = source();
            let mut result = String::with_capacity(formatter.len() + parser.len() + 1);
            result.push_str(formatter);
            result.push('\n');
            result.push_str(parser);
            result
        })
        .as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "kiln-native-number-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn compile_and_run(harness: &str) -> Option<std::process::Output> {
        compile_source_and_run(source(), harness, "number")
    }

    fn compile_source_and_run(
        unit: &str,
        harness: &str,
        stem: &str,
    ) -> Option<std::process::Output> {
        let cc = ["cc", "clang", "gcc"].into_iter().find(|name| {
            Command::new(name)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })?;
        let root = test_dir();
        std::fs::create_dir(&root).expect("create test directory");
        let source_path = root.join(format!("{stem}.c"));
        let harness_path = root.join("harness.c");
        let executable = root.join("probe");
        std::fs::write(&source_path, unit).expect("write number runtime source");
        std::fs::write(&harness_path, harness).expect("write formatter harness");
        let compile = Command::new(cc)
            .args([
                "-std=c11",
                "-pedantic",
                "-Wall",
                "-Wextra",
                "-Werror",
                // The upstream amalgamation keeps helpers for 32-bit and alternate-intrinsic
                // configurations in the same source; a given target intentionally leaves some
                // static inline functions unused.
                "-Wno-unused-function",
            ])
            .arg(&source_path)
            .arg(&harness_path)
            .arg("-lm")
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("invoke C compiler");
        assert!(
            compile.status.success(),
            "formatter failed to compile with {cc}:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&executable)
            .output()
            .expect("run formatter probe");
        let _ = std::fs::remove_dir_all(root);
        Some(run)
    }

    #[test]
    fn parser_source_matches_string_numeric_literal() {
        let harness = r#"
#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

double kiln_string_to_number(const char *, size_t);
int kiln_number_to_js_string(double, char *, size_t);

static int same_bits(double left, double right) {
    uint64_t left_bits;
    uint64_t right_bits;
    memcpy(&left_bits, &left, sizeof(left_bits));
    memcpy(&right_bits, &right, sizeof(right_bits));
    return left_bits == right_bits;
}

static int check(const char *text, double expected) {
    double actual = kiln_string_to_number(text, strlen(text));
    if (!same_bits(actual, expected)) {
        fprintf(stderr, "expected %.17g for %s, got %.17g\n", expected, text, actual);
        return 0;
    }
    return 1;
}

static int invalid(const char *text) {
    return isnan(kiln_string_to_number(text, strlen(text)));
}

int main(void) {
    static const char *spaces[] = {
        "\x09", "\x0a", "\x0b", "\x0c", "\x0d", "\x20", "\xc2\xa0",
        "\xe1\x9a\x80", "\xe2\x80\x80", "\xe2\x80\x81", "\xe2\x80\x82",
        "\xe2\x80\x83", "\xe2\x80\x84", "\xe2\x80\x85", "\xe2\x80\x86",
        "\xe2\x80\x87", "\xe2\x80\x88", "\xe2\x80\x89", "\xe2\x80\x8a",
        "\xe2\x80\xa8", "\xe2\x80\xa9", "\xe2\x80\xaf", "\xe2\x81\x9f",
        "\xe3\x80\x80", "\xef\xbb\xbf"
    };
    static const char *bad[] = {
        "NaN", "nan", "inf", "INFINITY", "1e", "1e+", "1_000", "1.2x",
        "Infinityx", "0x", "0b", "0o", "+0x10", "-0x10", "0x1p2", "0b2", "0o8"
    };
    char surrounded[8];
    char embedded_nul[] = {'1', '\0', '2'};
    char malformed[] = {(char)0xc0, (char)0xaf};
    char nel[] = {(char)0xc2, (char)0x85, '1'};
    char formatted[64];
    size_t index;

    if (!check("", 0.0) || !check(" \t\v\f\n\r", 0.0) ||
        !check("+1", 1.0) || !check("1.", 1.0) || !check(".5", 0.5) ||
        !check("01.25e+2", 125.0) || !check("-0", -0.0) || !check("-0e0", -0.0) ||
        !check("Infinity", INFINITY) || !check("+Infinity", INFINITY) ||
        !check("-Infinity", -INFINITY) || !check("1e309", INFINITY) ||
        !check("-1e309", -INFINITY) || !check("5e-324", 5e-324) ||
        !check("1e-324", 0.0) || !check("-1e-324", -0.0) ||
        !check("9007199254740993", 9007199254740992.0) ||
        !check("0x10", 16.0) || !check("0XfF", 255.0) ||
        !check("0b11", 3.0) || !check("0o10", 8.0) ||
        !check("0x1fffffffffffff", 9007199254740991.0) ||
        !check("0x20000000000001", 9007199254740992.0) ||
        !check("0x20000000000003", 9007199254740996.0) ||
        !check("0x3fffffffffffff", 18014398509481984.0)) return 1;

    for (index = 0; index < sizeof(spaces) / sizeof(spaces[0]); ++index) {
        size_t width = strlen(spaces[index]);
        memcpy(surrounded, spaces[index], width);
        surrounded[width] = '1';
        memcpy(surrounded + width + 1, spaces[index], width);
        surrounded[width * 2 + 1] = '\0';
        if (!check(surrounded, 1.0)) return 2;
    }
    for (index = 0; index < sizeof(bad) / sizeof(bad[0]); ++index) {
        if (!invalid(bad[index])) return 3;
    }
    if (!isnan(kiln_string_to_number(embedded_nul, sizeof(embedded_nul))) ||
        !isnan(kiln_string_to_number(malformed, sizeof(malformed))) ||
        !isnan(kiln_string_to_number(nel, sizeof(nel))) ||
        !isnan(kiln_string_to_number(NULL, 1)) ||
        !same_bits(kiln_string_to_number(NULL, 0), 0.0)) return 4;

    if (kiln_number_to_js_string(kiln_string_to_number("0.1", 3), formatted,
                                 sizeof(formatted)) != 3 ||
        strcmp(formatted, "0.1") != 0) return 5;
    return 0;
}
"#;
        let unit = runtime_source();
        let run = compile_source_and_run(unit, harness, "number-runtime")
            .expect("a C compiler is available");
        assert!(
            run.status.success(),
            "StringNumericLiteral probe failed: {}\n{}",
            run.status,
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn source_is_c11_and_matches_ecmascript_boundaries() {
        let harness = r#"
#include <stdint.h>
#include <stdio.h>
#include <string.h>

int kiln_number_to_js_string(double, char *, size_t);

static int check(double value, const char *expected) {
    char buffer[64];
    int length = kiln_number_to_js_string(value, buffer, sizeof(buffer));
    return length == (int)strlen(expected) && strcmp(buffer, expected) == 0;
}

int main(void) {
    return !(check(1e20, "100000000000000000000")
        && check(1e21, "1e+21")
        && check(1e-6, "0.000001")
        && check(1e-7, "1e-7")
        && check(0.1 + 0.2, "0.30000000000000004")
        && check(1.0 / 3.0, "0.3333333333333333")
        && check(2.0, "2")
        && check(9007199254740992.0, "9007199254740992")
        && check(123456789012345678901.0, "123456789012345680000")
        && check(5e-324, "5e-324")
        && check(1.7976931348623157e308, "1.7976931348623157e+308")
        && check(-0.0, "0")
        && check(0.0 / 0.0, "NaN")
        && check(1.0 / 0.0, "Infinity")
        && check(-1.0 / 0.0, "-Infinity"));
}
"#;
        let run = compile_and_run(harness).expect("a C compiler is available");
        assert!(
            run.status.success(),
            "formatter probe failed: {}\n{}",
            run.status,
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn source_is_locale_independent() {
        let harness = r#"
#include <locale.h>
#include <stddef.h>
#include <string.h>

int kiln_number_to_js_string(double, char *, size_t);

int main(void) {
    static const char *candidates[] = {
        "de_DE.UTF-8", "de_DE.utf8", "de_DE", "fr_FR.UTF-8", "fr_FR.utf8", "fr_FR",
        "it_IT.UTF-8", "es_ES.UTF-8", "pt_BR.UTF-8"
    };
    char buffer[64];
    size_t index;
    int found = 0;
    for (index = 0; index < sizeof(candidates) / sizeof(candidates[0]); ++index) {
        struct lconv *conventions;
        if (setlocale(LC_NUMERIC, candidates[index]) == NULL) continue;
        conventions = localeconv();
        if (conventions != NULL && strcmp(conventions->decimal_point, ",") == 0) {
            found = 1;
            break;
        }
    }
    if (!found) return 77;
    if (kiln_number_to_js_string(1.5, buffer, sizeof(buffer)) != 3) return 1;
    return strcmp(buffer, "1.5") == 0 ? 0 : 2;
}
"#;
        let run = compile_and_run(harness).expect("a C compiler is available");
        if run.status.code() == Some(77) {
            eprintln!("skipping hostile-locale probe: no comma-decimal locale is installed");
            return;
        }
        assert!(
            run.status.success(),
            "locale probe failed: {}\n{}",
            run.status,
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn source_matches_ecmascript_oracle_across_binary64_bits() {
        let mut harness = String::from(
            r#"
#include <stdint.h>
#include <stdio.h>
#include <string.h>

int kiln_number_to_js_string(double, char *, size_t);

struct case_value { uint64_t bits; const char *expected; };
static const struct case_value cases[] = {
"#,
        );
        let mut state = UINT64_C_SEED;
        let mut oracle = dragonbox_ecma::Buffer::new();
        for _ in 0..4096 {
            // SplitMix64 gives a deterministic, well-distributed sample over all bit patterns.
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut bits = state;
            bits = (bits ^ (bits >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            bits = (bits ^ (bits >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            bits ^= bits >> 31;
            let expected = oracle.format(f64::from_bits(bits));
            writeln!(harness, "    {{ UINT64_C({bits}), \"{expected}\" }},")
                .expect("write oracle case");
        }
        harness.push_str(
            r#"};

int main(void) {
    size_t index;
    for (index = 0; index < sizeof(cases) / sizeof(cases[0]); ++index) {
        char actual[64];
        double value;
        int length;
        memcpy(&value, &cases[index].bits, sizeof(value));
        length = kiln_number_to_js_string(value, actual, sizeof(actual));
        if (length < 0 || strcmp(actual, cases[index].expected) != 0) {
            fprintf(stderr, "case %zu: expected %s, got %s\n",
                    index, cases[index].expected, length < 0 ? "<error>" : actual);
            return 1;
        }
    }
    return 0;
}
"#,
        );
        let run = compile_and_run(&harness).expect("a C compiler is available");
        assert!(
            run.status.success(),
            "formatter disagreed with the ECMAScript oracle: {}\n{}",
            run.status,
            String::from_utf8_lossy(&run.stderr)
        );
    }

    const UINT64_C_SEED: u64 = 0x6b69_6c6e_2d72_7975;
}
