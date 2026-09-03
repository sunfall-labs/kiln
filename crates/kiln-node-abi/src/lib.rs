//! Declarative Node builtin ABI shared by every Kiln execution engine.
//!
//! Parsing does not decide what CommonJS receives from `require("node:events")`: that is a Node
//! runtime contract. Keeping the contract in this leaf crate lets the interpreter, graph linker,
//! and native materializer consume one table without depending on one another.

#![deny(clippy::panic, clippy::unwrap_used)]

/// How CommonJS projects a builtin's canonical export record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommonJsProjection {
    /// `module.exports` is a namespace object containing the canonical export record.
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

const ASSERT_PROPERTIES: &[(&str, &str)] = &[("ok", "ok"), ("strict", "strict")];

const BUILTINS: &[BuiltinAbi] = &[
    BuiltinAbi {
        specifier: "node:assert",
        exports: &["default", "ok", "strict"],
        commonjs: CommonJsProjection::Callable {
            export: "default",
            properties: ASSERT_PROPERTIES,
        },
    },
    BuiltinAbi {
        specifier: "node:async_hooks",
        exports: &["AsyncLocalStorage", "AsyncResource", "default"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:events",
        exports: &["EventEmitter", "default"],
        commonjs: CommonJsProjection::Callable {
            export: "EventEmitter",
            properties: EVENTS_PROPERTIES,
        },
    },
    BuiltinAbi {
        // `require("node:buffer")` is *both* the module and the `Buffer` constructor here: the
        // interpreter answers it with a value whose members are `Buffer`'s own statics, so
        // `require("node:buffer").from` is a function there. Declaring only `Buffer`, `constants`
        // and `default` is what let the native projection publish three names and answer
        // `undefined` for the other seven -- a silent wrong answer of exactly the shape a
        // `typeof buffer.from === "function"` feature test walks past. The statics are named here
        // so that one list drives the ESM bindings, the CommonJS namespace and the native slot
        // contract, and neither engine can grow a name the other does not see.
        specifier: "node:buffer",
        exports: &[
            "Buffer",
            "alloc",
            "allocUnsafe",
            "allocUnsafeSlow",
            "byteLength",
            "concat",
            "constants",
            "default",
            "from",
            "isBuffer",
        ],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:crypto",
        exports: &["createHash", "default", "randomUUID"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:diagnostics_channel",
        exports: &["channel", "default", "hasSubscribers", "tracingChannel"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:dns",
        exports: &["default", "lookup"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:fs/promises",
        exports: &["readFile", "stat", "writeFile"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:fs",
        exports: &[
            "accessSync",
            "appendFileSync",
            "closeSync",
            "copyFileSync",
            "default",
            "existsSync",
            "lstatSync",
            "mkdirSync",
            "openSync",
            "promises",
            "readFileSync",
            "readSync",
            "readdirSync",
            "readlinkSync",
            "realpathSync",
            "renameSync",
            "rmSync",
            "statSync",
            "unlinkSync",
            "writeFileSync",
        ],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:net",
        exports: &["connect", "createConnection", "createServer", "default"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:http",
        exports: &[
            "IncomingMessage",
            "METHODS",
            "Server",
            "ServerResponse",
            "createServer",
            "default",
            "get",
            "request",
        ],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:https",
        exports: &["createServer", "default"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:http2",
        exports: &["createSecureServer", "createServer", "default"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        // `require("node:module")` is the `Module` class, not a namespace: node answers
        // `typeof` "function" and `.name` "Module", and `Module.createRequire` hangs off it.
        // Measured against node v25.9.0, one probe per specifier -- the same measurement that
        // put `module` and `stream` on the interpreter's callable side of `typeof`. Declaring
        // the projection here is what lets the native backend agree: it published a fresh
        // namespace object for this name, so `typeof require("node:module")` was "object" in
        // one engine and "function" in the other, from the same bytecode.
        specifier: "node:module",
        exports: &["createRequire", "default"],
        commonjs: CommonJsProjection::Callable {
            export: "default",
            properties: &[],
        },
    },
    BuiltinAbi {
        specifier: "node:path",
        exports: &[
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
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:worker_threads",
        exports: &["Worker", "default"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:perf_hooks",
        exports: &["default", "performance"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:os",
        exports: &[
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
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:tty",
        exports: &["default", "isatty"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:zlib",
        exports: &[
            "createBrotliDecompress",
            "createGunzip",
            "createInflate",
            "default",
        ],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        // `require("node:stream")` is the `Stream` class for the same reason `node:module` is
        // `Module`: node answers "function", names it "Stream", and hangs `Readable`, `Writable`,
        // `addAbortSignal` and `finished` off it. Both engines already build that callable as
        // the module's `default`; only the CommonJS projection disagreed, and it disagreed by
        // wrapping the callable in a namespace object nobody can call.
        specifier: "node:stream",
        exports: &[
            "Readable",
            "Writable",
            "addAbortSignal",
            "default",
            "finished",
        ],
        commonjs: CommonJsProjection::Callable {
            export: "default",
            properties: &[],
        },
    },
    BuiltinAbi {
        specifier: "node:string_decoder",
        exports: &["StringDecoder", "default"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:querystring",
        exports: &["decode", "default", "parse"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:url",
        exports: &["URL", "default", "fileURLToPath", "parse", "pathToFileURL"],
        commonjs: CommonJsProjection::Namespace,
    },
    BuiltinAbi {
        specifier: "node:util",
        exports: &[
            "TextDecoder",
            "TextEncoder",
            "debuglog",
            "default",
            "deprecate",
            "format",
            "inherits",
            "inspect",
        ],
        commonjs: CommonJsProjection::Namespace,
    },
];

/// Every builtin whose resolver identity participates in the Node ABI.
pub const SPECIFIERS: &[&str] = &[
    "node:assert",
    "node:buffer",
    "node:async_hooks",
    "node:diagnostics_channel",
    "node:crypto",
    "node:dns",
    "node:events",
    "node:fs/promises",
    "node:fs",
    "node:net",
    "node:http",
    "node:module",
    "node:path",
    "node:worker_threads",
    "node:http2",
    "node:https",
    "node:perf_hooks",
    "node:os",
    "node:tty",
    "node:zlib",
    "node:stream",
    "node:string_decoder",
    "node:querystring",
    "node:url",
    "node:util",
];

/// Node's public HTTP method catalogue, which `require("node:http").METHODS` answers with.
///
/// Semantic data rather than anything sourced from a parser: Hyper parses method tokens, while
/// this list is a JavaScript compatibility contract. It lives here rather than in either engine
/// because both have to answer it and they have to answer it identically -- the native backend
/// published a placeholder callable for this name while the interpreter published the array, so
/// `typeof http.METHODS` differed between two runs of the same bytecode.
pub const HTTP_METHODS: &[&str] = &[
    "ACL",
    "BIND",
    "CHECKOUT",
    "CONNECT",
    "COPY",
    "DELETE",
    "GET",
    "HEAD",
    "LINK",
    "LOCK",
    "M-SEARCH",
    "MERGE",
    "MKACTIVITY",
    "MKCALENDAR",
    "MKCOL",
    "MOVE",
    "NOTIFY",
    "OPTIONS",
    "PATCH",
    "POST",
    "PROPFIND",
    "PROPPATCH",
    "PURGE",
    "PUT",
    // node v25.9.0 publishes 35 methods; this list had 34 because `QUERY` (RFC 9110's safe,
    // body-carrying search method) was added to llhttp after the list was copied. One word here
    // reaches both engines: the interpreter reads this slice and the native materializer writes
    // its C array out of it.
    "QUERY",
    "REBIND",
    "REPORT",
    "SEARCH",
    "SOURCE",
    "SUBSCRIBE",
    "TRACE",
    "UNBIND",
    "UNLINK",
    "UNLOCK",
    "UNSUBSCRIBE",
];

/// Look up a builtin with or without the canonical `node:` prefix.
#[must_use]
pub fn builtin_abi(specifier: &str) -> Option<&'static BuiltinAbi> {
    let bare = specifier.strip_prefix("node:").unwrap_or(specifier);
    let bare = if bare == "assert/strict" {
        "assert"
    } else {
        bare
    };
    BUILTINS
        .iter()
        .find(|builtin| builtin.specifier.strip_prefix("node:") == Some(bare))
}

impl CommonJsProjection {
    /// The canonical ESM export whose value becomes CommonJS `module.exports`, when callable.
    #[must_use]
    pub const fn callable_export(self) -> Option<&'static str> {
        match self {
            Self::Namespace => None,
            Self::Callable { export, .. } => Some(export),
        }
    }
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

    /// `node:buffer` declares the constructor's statics, not only the constructor.
    ///
    /// It used to declare three names, and this test pinned them. That was the hole: this realm
    /// answers `require("node:buffer").from` with a function -- the module and the constructor are
    /// the same thing here -- so a three-name list meant the native projection published three
    /// names and read `undefined` for the other seven. A silent `undefined` is what
    /// `typeof buffer.from === "function"` walks past, which is the one wrong answer the compiled
    /// backend is not allowed to give.
    #[test]
    fn buffer_commonjs_receives_the_namespace() {
        let buffer = builtin_abi("buffer").expect("buffer ABI");
        assert_eq!(
            buffer.exports,
            [
                "Buffer",
                "alloc",
                "allocUnsafe",
                "allocUnsafeSlow",
                "byteLength",
                "concat",
                "constants",
                "default",
                "from",
                "isBuffer",
            ]
        );
        assert_eq!(buffer.commonjs, CommonJsProjection::Namespace);
    }

    #[test]
    fn every_projection_names_one_canonical_export() {
        assert_eq!(BUILTINS.len(), SPECIFIERS.len());
        for specifier in SPECIFIERS {
            let abi = builtin_abi(specifier).expect("every resolver builtin has ABI metadata");
            if let Some(export) = abi.commonjs.callable_export() {
                assert!(abi.exports.contains(&export), "{specifier}");
            }
            if let CommonJsProjection::Callable { properties, .. } = abi.commonjs {
                for (property, export) in properties {
                    assert!(!property.is_empty(), "{specifier}");
                    assert!(abi.exports.contains(export), "{specifier}: {property}");
                }
            }
        }
    }

    #[test]
    fn resolver_aliases_share_the_canonical_abi() {
        assert_eq!(builtin_abi("assert/strict"), builtin_abi("node:assert"));
        assert_eq!(
            builtin_abi("node:assert/strict"),
            builtin_abi("node:assert")
        );
    }
}
