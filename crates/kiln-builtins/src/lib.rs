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
    Events {
        variant: Events,
        file: "node/events.ts",
        logical_path: "node:events.ts",
        source_const: EVENTS_SOURCE,
        exports_const: EVENTS_EXPORTS,
        sample_source_check: "export class EventEmitter",
        presence_test: events_builtin_source_is_present,
        compilation_test: events_builtin_compiles_cleanly,
        exports: [
            "EventEmitter",
            "default",
        ],
    },
    Buffer {
        variant: Buffer,
        file: "node/buffer.ts",
        logical_path: "node:buffer.ts",
        source_const: BUFFER_SOURCE,
        exports_const: BUFFER_EXPORTS,
        sample_source_check: "export const Buffer",
        presence_test: buffer_builtin_source_is_present,
        compilation_test: buffer_builtin_compiles_cleanly,
        exports: [
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
    },
    StringDecoder {
        variant: StringDecoder,
        file: "node/string_decoder.ts",
        logical_path: "node:string_decoder.ts",
        source_const: STRING_DECODER_SOURCE,
        exports_const: STRING_DECODER_EXPORTS,
        sample_source_check: "export class StringDecoder",
        presence_test: string_decoder_builtin_source_is_present,
        compilation_test: string_decoder_builtin_compiles_cleanly,
        exports: [
            "StringDecoder",
            "default",
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
    QueryString {
        variant: QueryString,
        file: "node/querystring.ts",
        logical_path: "node:querystring.ts",
        source_const: QUERYSTRING_SOURCE,
        exports_const: QUERYSTRING_EXPORTS,
        sample_source_check: "export function parse",
        presence_test: querystring_builtin_source_is_present,
        compilation_test: querystring_builtin_compiles_cleanly,
        exports: [
            "decode",
            "default",
            "encode",
            "escape",
            "parse",
            "stringify",
            "unescape",
        ],
    },
    Util {
        variant: Util,
        file: "node/util.ts",
        logical_path: "node:util.ts",
        source_const: UTIL_SOURCE,
        exports_const: UTIL_EXPORTS,
        sample_source_check: "export function format",
        presence_test: util_builtin_source_is_present,
        compilation_test: util_builtin_compiles_cleanly,
        exports: [
            "TextDecoder",
            "TextEncoder",
            "callbackify",
            "debuglog",
            "default",
            "deprecate",
            "format",
            "formatWithOptions",
            "inherits",
            "inspect",
            "isArray",
            "isBoolean",
            "isBuffer",
            "isDate",
            "isError",
            "isFunction",
            "isNull",
            "isNullOrUndefined",
            "isNumber",
            "isObject",
            "isPrimitive",
            "isRegExp",
            "isString",
            "isSymbol",
            "isUndefined",
            "promisify",
            "types",
        ],
    },
    Url {
        variant: Url,
        file: "node/url.ts",
        logical_path: "node:url.ts",
        source_const: URL_SOURCE,
        exports_const: URL_EXPORTS,
        sample_source_check: "export class URL",
        presence_test: url_builtin_source_is_present,
        compilation_test: url_builtin_compiles_cleanly,
        exports: [
            "URL",
            "URLSearchParams",
            "default",
            "domainToASCII",
            "domainToUnicode",
            "fileURLToPath",
            "format",
            "parse",
            "pathToFileURL",
            "urlToHttpOptions",
        ],
    },
    DiagnosticsChannel {
        variant: DiagnosticsChannel,
        file: "node/diagnostics_channel.ts",
        logical_path: "node:diagnostics_channel.ts",
        source_const: DIAGNOSTICS_CHANNEL_SOURCE,
        exports_const: DIAGNOSTICS_CHANNEL_EXPORTS,
        sample_source_check: "export class Channel",
        presence_test: diagnostics_channel_builtin_source_is_present,
        compilation_test: diagnostics_channel_builtin_compiles_cleanly,
        exports: [
            "Channel",
            "TracingChannel",
            "channel",
            "default",
            "hasSubscribers",
            "subscribe",
            "tracingChannel",
            "unsubscribe",
        ],
    },
    PerfHooks {
        variant: PerfHooks,
        file: "node/perf_hooks.ts",
        logical_path: "node:perf_hooks.ts",
        source_const: PERF_HOOKS_SOURCE,
        exports_const: PERF_HOOKS_EXPORTS,
        sample_source_check: "export class PerformanceEntry",
        presence_test: perf_hooks_builtin_source_is_present,
        compilation_test: perf_hooks_builtin_compiles_cleanly,
        exports: [
            "Performance",
            "PerformanceEntry",
            "PerformanceMark",
            "PerformanceMeasure",
            "PerformanceObserver",
            "PerformanceObserverEntryList",
            "constants",
            "createHistogram",
            "default",
            "eventLoopUtilization",
            "monitorEventLoopDelay",
            "performance",
            "timerify",
        ],
    },
    Console {
        variant: Console,
        file: "node/console.ts",
        logical_path: "node:console.ts",
        source_const: CONSOLE_SOURCE,
        exports_const: CONSOLE_EXPORTS,
        sample_source_check: "export const Console",
        presence_test: console_builtin_source_is_present,
        compilation_test: console_builtin_compiles_cleanly,
        exports: [
            "Console",
            "assert",
            "clear",
            "count",
            "countReset",
            "debug",
            "default",
            "dir",
            "dirxml",
            "error",
            "group",
            "groupCollapsed",
            "groupEnd",
            "info",
            "log",
            "profile",
            "profileEnd",
            "table",
            "time",
            "timeEnd",
            "timeLog",
            "timeStamp",
            "trace",
            "warn",
        ],
    },
    Tty {
        variant: Tty,
        file: "node/tty.ts",
        logical_path: "node:tty.ts",
        source_const: TTY_SOURCE,
        exports_const: TTY_EXPORTS,
        sample_source_check: "export function isatty",
        presence_test: tty_builtin_source_is_present,
        compilation_test: tty_builtin_compiles_cleanly,
        exports: [
            "ReadStream",
            "WriteStream",
            "default",
            "isatty",
        ],
    },
    WorkerThreads {
        variant: WorkerThreads,
        file: "node/worker_threads.ts",
        logical_path: "node:worker_threads.ts",
        source_const: WORKER_THREADS_SOURCE,
        exports_const: WORKER_THREADS_EXPORTS,
        sample_source_check: "export class Worker",
        presence_test: worker_threads_builtin_source_is_present,
        compilation_test: worker_threads_builtin_compiles_cleanly,
        exports: [
            "BroadcastChannel",
            "MessageChannel",
            "MessagePort",
            "SHARE_ENV",
            "Worker",
            "default",
            "getEnvironmentData",
            "isMainThread",
            "markAsUncloneable",
            "markAsUntransferable",
            "moveMessagePortToContext",
            "parentPort",
            "receiveMessageOnPort",
            "resourceLimits",
            "setEnvironmentData",
            "threadId",
            "workerData",
        ],
    },
    Module {
        variant: Module,
        file: "node/module.ts",
        logical_path: "node:module.ts",
        source_const: MODULE_SOURCE,
        exports_const: MODULE_EXPORTS,
        sample_source_check: "export class Module",
        presence_test: module_builtin_source_is_present,
        compilation_test: module_builtin_compiles_cleanly,
        exports: [
            "Module",
            "builtinModules",
            "createRequire",
            "default",
            "isBuiltin",
        ],
    },
    Dns {
        variant: Dns,
        file: "node/dns.ts",
        logical_path: "node:dns.ts",
        source_const: DNS_SOURCE,
        exports_const: DNS_EXPORTS,
        sample_source_check: "export function lookup",
        presence_test: dns_builtin_source_is_present,
        compilation_test: dns_builtin_compiles_cleanly,
        exports: [
            "ADDRCONFIG",
            "ADDRGETNETWORKPARAMS",
            "ALL",
            "BADFAMILY",
            "BADFLAGS",
            "BADHINTS",
            "BADNAME",
            "BADQUERY",
            "BADRESP",
            "BADSTR",
            "CANCELLED",
            "CONNREFUSED",
            "DESTRUCTION",
            "EOF",
            "FILE",
            "FORMERR",
            "LOADIPHLPAPI",
            "NODATA",
            "NOMEM",
            "NONAME",
            "NOTFOUND",
            "NOTIMP",
            "NOTINITIALIZED",
            "REFUSED",
            "Resolver",
            "SERVFAIL",
            "TIMEOUT",
            "V4MAPPED",
            "default",
            "getDefaultResultOrder",
            "getServers",
            "lookup",
            "lookupService",
            "promises",
            "resolve",
            "resolve4",
            "resolve6",
            "resolveAny",
            "resolveCaa",
            "resolveCname",
            "resolveMx",
            "resolveNaptr",
            "resolveNs",
            "resolvePtr",
            "resolveSoa",
            "resolveSrv",
            "resolveTlsa",
            "resolveTxt",
            "reverse",
            "setDefaultResultOrder",
            "setServers",
        ],
    },
    Crypto {
        variant: Crypto,
        file: "node/crypto.ts",
        logical_path: "node:crypto.ts",
        source_const: CRYPTO_SOURCE,
        exports_const: CRYPTO_EXPORTS,
        sample_source_check: "export function createHash",
        presence_test: crypto_builtin_source_is_present,
        compilation_test: crypto_builtin_compiles_cleanly,
        exports: [
            "Hash",
            "Hmac",
            "createHash",
            "createHmac",
            "default",
            "getHashes",
            "getRandomValues",
            "randomBytes",
            "randomFillSync",
            "randomUUID",
            "timingSafeEqual",
        ],
    },
}
