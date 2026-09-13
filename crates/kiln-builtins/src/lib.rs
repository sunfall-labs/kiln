//! Typed TypeScript implementations of Kiln built-ins (ADR 0005).
//!
//! Provides the embedded TypeScript sources for Node built-ins,
//! compiled by Kiln's query pipeline to ensure 100% semantic parity
//! between interpreter and native execution.

use kiln_module_resolver::NodeBuiltin;

pub const OS_SOURCE: &str = include_str!("node/os.ts");

/// Return the TypeScript source code for a builtin if it has been migrated to TypeScript.
pub fn builtin_source(builtin: NodeBuiltin) -> Option<&'static str> {
    match builtin {
        NodeBuiltin::Os => Some(OS_SOURCE),
        _ => None,
    }
}

/// Whether the builtin is implemented as a TypeScript module.
pub fn is_ts_builtin(builtin: NodeBuiltin) -> bool {
    builtin_source(builtin).is_some()
}

pub const OS_EXPORTS: &[&str] = &[
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
];

/// Return the supported exports for a builtin if it is a TypeScript builtin.
pub fn builtin_supported_exports(builtin: NodeBuiltin) -> Option<&'static [&'static str]> {
    match builtin {
        NodeBuiltin::Os => Some(OS_EXPORTS),
        _ => None,
    }
}

/// Logical path with `.ts` extension for frontend parsing.
pub fn builtin_logical_path(builtin: NodeBuiltin) -> &'static str {
    match builtin {
        NodeBuiltin::Os => "node:os.ts",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_builtin_source_is_present() {
        assert!(is_ts_builtin(NodeBuiltin::Os));
        assert!(
            builtin_source(NodeBuiltin::Os)
                .unwrap()
                .contains("export function homedir")
        );
    }

    #[test]
    fn os_builtin_compiles_cleanly() {
        let logical_path = builtin_logical_path(NodeBuiltin::Os);
        let hir = frontend_oxc::parse(logical_path, OS_SOURCE);
        let program =
            kiln_ir::compile_module(&hir).expect("node:os should compile to IR without error");
        let exported_names: std::collections::BTreeSet<_> = program
            .exports
            .iter()
            .map(|e| e.exported.as_str())
            .collect();
        let expected = [
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
        ];
        for name in expected {
            assert!(
                exported_names.contains(name),
                "node:os program must export `{name}`"
            );
        }
    }
}
