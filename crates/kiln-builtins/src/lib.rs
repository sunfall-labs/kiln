//! Typed TypeScript implementations of Kiln built-ins (ADR 0005).
//!
//! Provides the embedded TypeScript sources for Node built-ins,
//! compiled by Kiln's query pipeline to ensure 100% semantic parity
//! between interpreter and native execution.

use kiln_module_resolver::NodeBuiltin;

macro_rules! declare_ts_builtins {
    ($(
        $name:ident {
            variant: $variant:ident,
            file: $file:literal,
            logical_path: $logical_path:literal,
            source_const: $source_const:ident,
            exports_const: $exports_const:ident,
            sample_source_check: $sample:literal,
            presence_test: $presence_test:ident,
            compilation_test: $compilation_test:ident,
            exports: [$($export:literal),* $(,)?],
        }
    ),* $(,)?) => {
        $(
            pub const $source_const: &str = include_str!($file);
            pub const $exports_const: &[&str] = &[
                $($export),*
            ];
        )*

        /// Return the TypeScript source code for a builtin if it has been migrated to TypeScript.
        pub fn builtin_source(builtin: NodeBuiltin) -> Option<&'static str> {
            match builtin {
                $(NodeBuiltin::$variant => Some($source_const),)*
                _ => None,
            }
        }

        /// Whether the builtin is implemented as a TypeScript module.
        pub fn is_ts_builtin(builtin: NodeBuiltin) -> bool {
            builtin_source(builtin).is_some()
        }

        /// Return the supported exports for a builtin if it is a TypeScript builtin.
        pub fn builtin_supported_exports(builtin: NodeBuiltin) -> Option<&'static [&'static str]> {
            match builtin {
                $(NodeBuiltin::$variant => Some($exports_const),)*
                _ => None,
            }
        }

        /// Logical path with `.ts` extension for frontend parsing.
        pub fn builtin_logical_path(builtin: NodeBuiltin) -> &'static str {
            match builtin {
                $(NodeBuiltin::$variant => $logical_path,)*
                _ => "",
            }
        }

        #[cfg(test)]
        mod tests {
            use super::*;

            $(
                #[test]
                fn $presence_test() {
                    assert!(is_ts_builtin(NodeBuiltin::$variant));
                    assert!(
                        builtin_source(NodeBuiltin::$variant)
                            .unwrap()
                            .contains($sample)
                    );
                }

                #[test]
                fn $compilation_test() {
                    let logical_path = builtin_logical_path(NodeBuiltin::$variant);
                    let hir = frontend_oxc::parse(logical_path, $source_const);
                    let program = kiln_ir::compile_module(&hir)
                        .expect(concat!(stringify!($name), " should compile to IR without error"));
                    let exported_names: std::collections::BTreeSet<_> = program
                        .exports
                        .iter()
                        .map(|e| e.exported.as_str())
                        .collect();
                    for name in $exports_const {
                        assert!(
                            exported_names.contains(name),
                            concat!(stringify!($name), " program must export `{}`"),
                            name
                        );
                    }
                }
            )*
        }
    };
}

declare_ts_builtins! {
    Assert {
        variant: Assert,
        file: "node/assert.ts",
        logical_path: "node:assert.ts",
        source_const: ASSERT_SOURCE,
        exports_const: ASSERT_EXPORTS,
        sample_source_check: "export function ok",
        presence_test: assert_builtin_source_is_present,
        compilation_test: assert_builtin_compiles_cleanly,
        exports: [
            "AssertionError",
            "deepEqual",
            "deepStrictEqual",
            "default",
            "doesNotMatch",
            "doesNotReject",
            "doesNotThrow",
            "equal",
            "fail",
            "ifError",
            "match",
            "notDeepEqual",
            "notDeepStrictEqual",
            "notEqual",
            "notStrictEqual",
            "ok",
            "rejects",
            "strict",
            "strictEqual",
            "throws",
        ],
    },
    Os {
        variant: Os,
        file: "node/os.ts",
        logical_path: "node:os.ts",
        source_const: OS_SOURCE,
        exports_const: OS_EXPORTS,
        sample_source_check: "export function homedir",
        presence_test: os_builtin_source_is_present,
        compilation_test: os_builtin_compiles_cleanly,
        exports: [
            "EOL",
            "arch",
            "availableParallelism",
            "cpus",
            "default",
            "devNull",
            "endianness",
            "freemem",
            "homedir",
            "hostname",
            "loadavg",
            "machine",
            "networkInterfaces",
            "platform",
            "release",
            "tmpdir",
            "totalmem",
            "type",
            "uptime",
            "userInfo",
            "version",
        ],
    },
    Path {
        variant: Path,
        file: "node/path.ts",
        logical_path: "node:path.ts",
        source_const: PATH_SOURCE,
        exports_const: PATH_EXPORTS,
        sample_source_check: "export function join",
        presence_test: path_builtin_source_is_present,
        compilation_test: path_builtin_compiles_cleanly,
        exports: [
            "basename",
            "default",
            "delimiter",
            "dirname",
            "extname",
            "format",
            "isAbsolute",
            "join",
            "normalize",
            "parse",
            "posix",
            "relative",
            "resolve",
            "sep",
        ],
    },
}
