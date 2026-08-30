//! ECMAScript `Number::toString` formatting for the native C backend.
//!
//! The implementation returned by [`source`] is a deterministic C11 translation unit fragment.
//! It contains a namespaced, static copy of Ulf Adams' Ryu binary64 shortest-conversion routine
//! and a small adapter implementing the ECMAScript decimal/exponential presentation rules.

/// Version of the vendored formatter source and its C ABI.
pub const NUMBER_FORMAT_VERSION: u32 = 1;

/// Return the deterministic C11 formatter source.
pub fn source() -> &'static str {
    include_str!("../c/kiln_number.c")
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
        let source_path = root.join("number.c");
        let harness_path = root.join("harness.c");
        let executable = root.join("probe");
        std::fs::write(&source_path, source()).expect("write formatter source");
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
