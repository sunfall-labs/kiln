//! Declarative Node builtin ABI shared by every Kiln execution engine.
//!
//! Parsing does not decide what CommonJS receives from `require("node:events")`: that is a Node
//! runtime contract. Keeping the contract in this leaf crate lets the interpreter, graph linker,
//! and native materializer consume one table without depending on one another.

#![deny(clippy::panic, clippy::unwrap_used)]

/// How CommonJS projects a builtin's canonical export record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommonJsProjection {
    /// `module.exports` is the builtin namespace object.
    Namespace,
    /// `module.exports` is one named callable export, decorated with the listed aliases.
    Callable {
        export: &'static str,
        properties: &'static [(&'static str, &'static str)],
    },
}

/// The stable public shape of one Node builtin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinAbi {
    pub specifier: &'static str,
    pub exports: &'static [&'static str],
    pub commonjs: CommonJsProjection,
}

const EVENTS_PROPERTIES: &[(&str, &str)] = &[
    ("EventEmitter", "EventEmitter"),
    ("default", "EventEmitter"),
];

const BUILTINS: &[BuiltinAbi] = &[BuiltinAbi {
    specifier: "node:events",
    exports: &["EventEmitter", "default"],
    commonjs: CommonJsProjection::Callable {
        export: "EventEmitter",
        properties: EVENTS_PROPERTIES,
    },
}];

/// Look up a builtin with or without the canonical `node:` prefix.
#[must_use]
pub fn builtin_abi(specifier: &str) -> Option<&'static BuiltinAbi> {
    let bare = specifier.strip_prefix("node:").unwrap_or(specifier);
    BUILTINS
        .iter()
        .find(|builtin| builtin.specifier.strip_prefix("node:") == Some(bare))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_has_one_canonical_esm_and_commonjs_shape() {
        let events = builtin_abi("events").expect("events ABI");
        assert_eq!(events, builtin_abi("node:events").expect("prefixed ABI"));
        assert_eq!(events.exports, ["EventEmitter", "default"]);
        assert_eq!(
            events.commonjs,
            CommonJsProjection::Callable {
                export: "EventEmitter",
                properties: EVENTS_PROPERTIES,
            }
        );
    }
}
