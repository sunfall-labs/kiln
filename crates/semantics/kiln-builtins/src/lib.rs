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

        /// Return the authoritative TypeScript source for every resolver builtin.
        pub fn builtin_source(builtin: NodeBuiltin) -> &'static str {
            match builtin {
                $(NodeBuiltin::$variant => $source_const,)*
            }
        }

        /// Whether a specifier names one of the resolver's builtin modules.
        pub fn is_builtin_specifier(specifier: &str) -> bool {
            NodeBuiltin::parse(specifier).is_some()
        }

        /// The canonical specifiers whose values come from graph evaluation.
        pub fn ts_builtin_specifiers() -> Vec<&'static str> {
            NodeBuiltin::ALL
                .iter()
                .copied()
                .map(NodeBuiltin::specifier)
                .collect()
        }

        /// Return the supported exports for every resolver builtin.
        pub fn builtin_supported_exports(builtin: NodeBuiltin) -> &'static [&'static str] {
            match builtin {
                $(NodeBuiltin::$variant => $exports_const,)*
            }
        }

        /// Return the authoritative TS exports for a builtin specifier, including bare aliases.
        pub fn builtin_exports_for_specifier(specifier: &str) -> Option<&'static [&'static str]> {
            NodeBuiltin::parse(specifier).map(builtin_supported_exports)
        }

        /// Logical path with `.ts` extension for frontend parsing.
        pub fn builtin_logical_path(builtin: NodeBuiltin) -> &'static str {
            match builtin {
                $(NodeBuiltin::$variant => $logical_path,)*
            }
        }

        #[cfg(test)]
        mod tests {
            use super::*;

            $(
                #[test]
                fn $presence_test() {
                    assert!(builtin_source(NodeBuiltin::$variant).contains($sample));
                }

                #[test]
                fn $compilation_test() {
                    let logical_path = builtin_logical_path(NodeBuiltin::$variant);
                    let hir = kiln_frontend_oxc::parse(logical_path, $source_const);
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
    Actors {
        variant: Actors,
        file: "actors.ts",
        logical_path: "kiln:actors.ts",
        source_const: ACTORS_SOURCE,
        exports_const: ACTORS_EXPORTS,
        sample_source_check: "export async function spawnActor",
        presence_test: actors_builtin_source_is_present,
        compilation_test: actors_builtin_compiles_cleanly,
        exports: ["Actor", "spawnActor", "connectActor", "isolate", "prepareTransfer", "transfer"],
    },
    ActorsCore {
        variant: ActorsCore,
        file: "actors_core.ts",
        logical_path: "kiln:actors/core.ts",
        source_const: ACTORS_CORE_SOURCE,
        exports_const: ACTORS_CORE_EXPORTS,
        sample_source_check: "export abstract class Actor",
        presence_test: actors_core_builtin_source_is_present,
        compilation_test: actors_core_builtin_compiles_cleanly,
        exports: ["Actor", "isolate", "prepareTransfer", "transfer", "prepareActorMessage", "consumeActorTickets"],
    },
    ActorsWorker {
        variant: ActorsWorker,
        file: "actors_worker.ts",
        logical_path: "kiln:actors/worker.ts",
        source_const: ACTORS_WORKER_SOURCE,
        exports_const: ACTORS_WORKER_EXPORTS,
        sample_source_check: "export const actorWorkerEntry",
        presence_test: actors_worker_builtin_source_is_present,
        compilation_test: actors_worker_builtin_compiles_cleanly,
        exports: ["actorWorkerEntry"],
    },
    Shared {
        variant: Shared,
        file: "shared.ts",
        logical_path: "kiln:shared.ts",
        source_const: SHARED_SOURCE,
        exports_const: SHARED_EXPORTS,
        sample_source_check: "export function share",
        presence_test: shared_builtin_source_is_present,
        compilation_test: shared_builtin_compiles_cleanly,
        exports: ["default", "share", "isShared"],
    },
    Gui {
        variant: Gui,
        file: "gui.ts",
        logical_path: "kiln:gui.ts",
        source_const: GUI_SOURCE,
        exports_const: GUI_EXPORTS,
        sample_source_check: "export class Window",
        presence_test: gui_builtin_source_is_present,
        compilation_test: gui_builtin_compiles_cleanly,
        exports: [
            "Button",
            "Fragment",
            "HStack",
            "HostGuiBridge",
            "Menu",
            "MenuItem",
            "MenuSeparator",
            "Node",
            "Slider",
            "Submenu",
            "Text",
            "TextInput",
            "Toggle",
            "TrayIcon",
            "VStack",
            "Window",
            "app",
            "batch",
            "clipboard",
            "createElement",
            "createEffect",
            "createMemo",
            "createRenderer",
            "createSignal",
            "default",
            "dialog",
            "getActiveDriver",
            "globalShortcut",
            "gtk",
            "h",
            "hostGuiBridge",
            "nodeRegistry",
            "notification",
            "onCleanup",
            "runLoop",
            "systemTheme",
            "turnMailbox",
            "untrack",
        ],
    },
    Process {
        variant: Process,
        file: "node/process.ts",
        logical_path: "kiln:process.ts",
        source_const: PROCESS_SOURCE,
        exports_const: PROCESS_EXPORTS,
        sample_source_check: "export async function run",
        presence_test: process_builtin_source_is_present,
        compilation_test: process_builtin_compiles_cleanly,
        exports: [
            "arch", "argv", "cwd", "default", "env", "exit", "pid", "platform", "run",
            "versions",
        ],
    },
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
            "EventEmitterAsyncResource",
            "once",
            "on",
            "getEventListeners",
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
            "constants",
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
            "toNamespacedPath",
            "win32",
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
            "isDeepStrictEqual",
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
            "styleText",
            "stripVTControlCharacters",
            "types",
        ],
    },
    Url {
        variant: Url,
        file: "node/url.ts",
        logical_path: "node:url.ts",
        source_const: URL_SOURCE,
        exports_const: URL_EXPORTS,
        sample_source_check: "export const URL",
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
            "isInternalThread",
            "isMainThread",
            "isMarkedAsUntransferable",
            "markAsUncloneable",
            "markAsUntransferable",
            "moveMessagePortToContext",
            "parentPort",
            "postMessageToThread",
            "receiveMessageOnPort",
            "resourceLimits",
            "setEnvironmentData",
            "threadId",
            "threadName",
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
    Http2 {
        variant: Http2,
        file: "node/http2.ts",
        logical_path: "node:http2.ts",
        source_const: HTTP2_SOURCE,
        exports_const: HTTP2_EXPORTS,
        sample_source_check: "export function createServer",
        presence_test: http2_builtin_source_is_present,
        compilation_test: http2_builtin_compiles_cleanly,
        exports: [
            "connect",
            "constants",
            "createSecureServer",
            "createServer",
            "default",
            "getDefaultSettings",
        ],
    },
    Zlib {
        variant: Zlib,
        file: "node/zlib.ts",
        logical_path: "node:zlib.ts",
        source_const: ZLIB_SOURCE,
        exports_const: ZLIB_EXPORTS,
        sample_source_check: "export function createGzip",
        presence_test: zlib_builtin_source_is_present,
        compilation_test: zlib_builtin_compiles_cleanly,
        exports: [
            "constants",
            "createBrotliDecompress",
            "createDeflate",
            "createGunzip",
            "createGzip",
            "createInflate",
            "createUnzip",
            "default",
            "deflateSync",
            "gunzipSync",
            "gzipSync",
            "inflateSync",
            "unzipSync",
        ],
    },
    Stream {
        variant: Stream,
        file: "node/stream.ts",
        logical_path: "node:stream.ts",
        source_const: STREAM_SOURCE,
        exports_const: STREAM_EXPORTS,
        sample_source_check: "export class Readable",
        presence_test: stream_builtin_source_is_present,
        compilation_test: stream_builtin_compiles_cleanly,
        exports: [
            "Duplex",
            "PassThrough",
            "Readable",
            "Stream",
            "Transform",
            "Writable",
            "addAbortSignal",
            "default",
            "finished",
            "pipeline",
            "promises",
        ],
    },
    Https {
        variant: Https,
        file: "node/https.ts",
        logical_path: "node:https.ts",
        source_const: HTTPS_SOURCE,
        exports_const: HTTPS_EXPORTS,
        sample_source_check: "export function createServer",
        presence_test: https_builtin_source_is_present,
        compilation_test: https_builtin_compiles_cleanly,
        exports: [
            "createServer",
            "default",
        ],
    },
    AsyncHooks {
        variant: AsyncHooks,
        file: "node/async_hooks.ts",
        logical_path: "node:async_hooks.ts",
        source_const: ASYNC_HOOKS_SOURCE,
        exports_const: ASYNC_HOOKS_EXPORTS,
        sample_source_check: "export const AsyncLocalStorage",
        presence_test: async_hooks_builtin_source_is_present,
        compilation_test: async_hooks_builtin_compiles_cleanly,
        exports: [
            "AsyncLocalStorage",
            "AsyncResource",
            "default",
        ],
    },
    Net {
        variant: Net,
        file: "node/net.ts",
        logical_path: "node:net.ts",
        source_const: NET_SOURCE,
        exports_const: NET_EXPORTS,
        sample_source_check: "export function createServer",
        presence_test: net_builtin_source_is_present,
        compilation_test: net_builtin_compiles_cleanly,
        exports: [
            "connect",
            "createConnection",
            "createServer",
            "default",
            "isIP",
            "isIPv4",
            "isIPv6",
        ],
    },
    Http {
        variant: Http,
        file: "node/http.ts",
        logical_path: "node:http.ts",
        source_const: HTTP_SOURCE,
        exports_const: HTTP_EXPORTS,
        sample_source_check: "export function createServer",
        presence_test: http_builtin_source_is_present,
        compilation_test: http_builtin_compiles_cleanly,
        exports: [
            "IncomingMessage",
            "METHODS",
            "Server",
            "ServerResponse",
            "createServer",
            "default",
            "get",
            "request",
            "serve",
        ],
    },
    FsPromises {
        variant: FsPromises,
        file: "node/fs/promises.ts",
        logical_path: "node:fs/promises.ts",
        source_const: FS_PROMISES_SOURCE,
        exports_const: FS_PROMISES_EXPORTS,
        sample_source_check: "export const readFile",
        presence_test: fs_promises_builtin_source_is_present,
        compilation_test: fs_promises_builtin_compiles_cleanly,
        exports: [
            "FileHandle",
            "access",
            "appendFile",
            "chmod",
            "chown",
            "copyFile",
            "cp",
            "default",
            "glob",
            "link",
            "lstat",
            "mkdir",
            "mkdtemp",
            "open",
            "readFile",
            "readlink",
            "readdir",
            "realpath",
            "rename",
            "rm",
            "rmdir",
            "stat",
            "symlink",
            "truncate",
            "unlink",
            "utimes",
            "watch",
            "writeFile",
        ],
    },
    Fs {
        variant: Fs,
        file: "node/fs.ts",
        logical_path: "node:fs.ts",
        source_const: FS_SOURCE,
        exports_const: FS_EXPORTS,
        sample_source_check: "export function existsSync",
        presence_test: fs_builtin_source_is_present,
        compilation_test: fs_builtin_compiles_cleanly,
        exports: [
            "FSWatcher",
            "Stats",
            "accessSync",
            "appendFileSync",
            "chmodSync",
            "close",
            "closeSync",
            "constants",
            "copyFileSync",
            "createReadStream",
            "createWriteStream",
            "default",
            "existsSync",
            "fchmod",
            "fchmodSync",
            "fdatasyncSync",
            "fstatSync",
            "fsync",
            "fsyncSync",
            "ftruncateSync",
            "futimes",
            "futimesSync",
            "glob",
            "globSync",
            "lstat",
            "lstatSync",
            "mkdir",
            "mkdirSync",
            "mkdtempSync",
            "open",
            "openSync",
            "promises",
            "readFile",
            "readFileSync",
            "readSync",
            "readdirSync",
            "readlinkSync",
            "realpath",
            "realpathSync",
            "renameSync",
            "rmSync",
            "rmdir",
            "rmdirSync",
            "stat",
            "statSync",
            "truncateSync",
            "unlink",
            "unlinkSync",
            "unwatchFile",
            "utimes",
            "utimesSync",
            "watch",
            "watchFile",
            "write",
            "writeFile",
            "writeFileSync",
            "writeSync",
        ],
    },
    Ffi {
        variant: Ffi,
        file: "node/ffi.ts",
        logical_path: "node:ffi.ts",
        source_const: FFI_SOURCE,
        exports_const: FFI_EXPORTS,
        sample_source_check: "export function dlopen",
        presence_test: ffi_builtin_source_is_present,
        compilation_test: ffi_builtin_compiles_cleanly,
        exports: [
            "default",
            "dlopen",
            "getRawPointer",
            "suffix",
            "toArrayBuffer",
        ],
    },
    ChildProcess {
        variant: ChildProcess,
        file: "node/child_process.ts",
        logical_path: "node:child_process.ts",
        source_const: CHILD_PROCESS_SOURCE,
        exports_const: CHILD_PROCESS_EXPORTS,
        sample_source_check: "export class ChildProcess",
        presence_test: child_process_builtin_source_is_present,
        compilation_test: child_process_builtin_compiles_cleanly,
        exports: [
            "ChildProcess",
            "default",
            "exec",
            "execFile",
            "execFileSync",
            "execSync",
            "fork",
            "spawn",
            "spawnSync",
        ],
    },
    Timers {
        variant: Timers,
        file: "node/timers.ts",
        logical_path: "node:timers.ts",
        source_const: TIMERS_SOURCE,
        exports_const: TIMERS_EXPORTS,
        sample_source_check: "export const setTimeout",
        presence_test: timers_builtin_source_is_present,
        compilation_test: timers_builtin_compiles_cleanly,
        exports: [
            "clearInterval",
            "clearImmediate",
            "clearTimeout",
            "default",
            "setImmediate",
            "setInterval",
            "setTimeout",
        ],
    },
    TimersPromises {
        variant: TimersPromises,
        file: "node/timers/promises.ts",
        logical_path: "node:timers/promises.ts",
        source_const: TIMERS_PROMISES_SOURCE,
        exports_const: TIMERS_PROMISES_EXPORTS,
        sample_source_check: "export function setTimeout",
        presence_test: timers_promises_builtin_source_is_present,
        compilation_test: timers_promises_builtin_compiles_cleanly,
        exports: [
            "default",
            "setImmediate",
            "setInterval",
            "setTimeout",
        ],
    },
    Readline {
        variant: Readline,
        file: "node/readline.ts",
        logical_path: "node:readline.ts",
        source_const: READLINE_SOURCE,
        exports_const: READLINE_EXPORTS,
        sample_source_check: "export class Interface",
        presence_test: readline_builtin_source_is_present,
        compilation_test: readline_builtin_compiles_cleanly,
        exports: [
            "Interface",
            "clearLine",
            "clearScreenDown",
            "createInterface",
            "cursorTo",
            "default",
            "emitKeypressEvents",
            "moveCursor",
            "promises",
        ],
    },
    ReadlinePromises {
        variant: ReadlinePromises,
        file: "node/readline/promises.ts",
        logical_path: "node:readline/promises.ts",
        source_const: READLINE_PROMISES_SOURCE,
        exports_const: READLINE_PROMISES_EXPORTS,
        sample_source_check: "export class Interface",
        presence_test: readline_promises_builtin_source_is_present,
        compilation_test: readline_promises_builtin_compiles_cleanly,
        exports: [
            "Interface",
            "createInterface",
            "default",
        ],
    },
    StreamPromises {
        variant: StreamPromises,
        file: "node/stream/promises.ts",
        logical_path: "node:stream/promises.ts",
        source_const: STREAM_PROMISES_SOURCE,
        exports_const: STREAM_PROMISES_EXPORTS,
        sample_source_check: "export function pipeline",
        presence_test: stream_promises_builtin_source_is_present,
        compilation_test: stream_promises_builtin_compiles_cleanly,
        exports: [
            "default",
            "finished",
            "pipeline",
        ],
    },
    Sqlite {
        variant: Sqlite,
        file: "node/sqlite.ts",
        logical_path: "node:sqlite.ts",
        source_const: SQLITE_SOURCE,
        exports_const: SQLITE_EXPORTS,
        sample_source_check: "export class DatabaseSync",
        presence_test: sqlite_builtin_source_is_present,
        compilation_test: sqlite_builtin_compiles_cleanly,
        exports: [
            "DatabaseSync",
            "StatementSync",
            "default",
        ],
    },
    UtilTypes {
        variant: UtilTypes,
        file: "node/util/types.ts",
        logical_path: "node:util/types.ts",
        source_const: UTIL_TYPES_SOURCE,
        exports_const: UTIL_TYPES_EXPORTS,
        sample_source_check: "export function isArrayBuffer",
        presence_test: util_types_builtin_source_is_present,
        compilation_test: util_types_builtin_compiles_cleanly,
        exports: [
            "default",
            "isAnyArrayBuffer",
            "isArgumentsObject",
            "isArrayBuffer",
            "isArrayBufferView",
            "isAsyncFunction",
            "isBigInt64Array",
            "isBigIntObject",
            "isBigUint64Array",
            "isBooleanObject",
            "isBoxedPrimitive",
            "isDataView",
            "isDate",
            "isFloat32Array",
            "isFloat64Array",
            "isGeneratorFunction",
            "isInt8Array",
            "isInt16Array",
            "isInt32Array",
            "isMap",
            "isNativeError",
            "isNumberObject",
            "isPromise",
            "isRegExp",
            "isSet",
            "isStringObject",
            "isSymbolObject",
            "isTypedArray",
            "isUint8Array",
            "isUint8ClampedArray",
            "isUint16Array",
            "isUint32Array",
            "isWeakMap",
            "isWeakSet",
        ],
    },
    StreamWeb {
        variant: StreamWeb,
        file: "node/stream/web.ts",
        logical_path: "node:stream/web.ts",
        source_const: STREAM_WEB_SOURCE,
        exports_const: STREAM_WEB_EXPORTS,
        sample_source_check: "export const ReadableStream",
        presence_test: stream_web_builtin_source_is_present,
        compilation_test: stream_web_builtin_compiles_cleanly,
        exports: [
            "ByteLengthQueuingStrategy",
            "CompressionStream",
            "CountQueuingStrategy",
            "DecompressionStream",
            "ReadableByteStreamController",
            "ReadableStream",
            "ReadableStreamBYOBReader",
            "ReadableStreamBYOBRequest",
            "ReadableStreamDefaultController",
            "ReadableStreamDefaultReader",
            "TextDecoderStream",
            "TextEncoderStream",
            "TransformStream",
            "TransformStreamDefaultController",
            "WritableStream",
            "WritableStreamDefaultController",
            "WritableStreamDefaultWriter",
            "default",
        ],
    },
    Constants {
        variant: Constants,
        file: "node/constants.ts",
        logical_path: "node:constants.ts",
        source_const: CONSTANTS_SOURCE,
        exports_const: CONSTANTS_EXPORTS,
        sample_source_check: "export const O_RDONLY",
        presence_test: constants_builtin_source_is_present,
        compilation_test: constants_builtin_compiles_cleanly,
        exports: [
            "COPYFILE_EXCL",
            "COPYFILE_FICLONE",
            "COPYFILE_FICLONE_FORCE",
            "E2BIG",
            "EACCES",
            "EADDRINUSE",
            "EADDRNOTAVAIL",
            "EAFNOSUPPORT",
            "EAGAIN",
            "EALREADY",
            "EBADF",
            "EBADMSG",
            "EBUSY",
            "ECANCELED",
            "ECHILD",
            "ECONNABORTED",
            "ECONNREFUSED",
            "ECONNRESET",
            "EDEADLK",
            "EDESTADDRREQ",
            "EDOM",
            "EDQUOT",
            "EEXIST",
            "EFAULT",
            "EFBIG",
            "EHOSTUNREACH",
            "EIDRM",
            "EILSEQ",
            "EINPROGRESS",
            "EINTR",
            "EINVAL",
            "EIO",
            "EISCONN",
            "EISDIR",
            "ELOOP",
            "EMFILE",
            "EMLINK",
            "EMSGSIZE",
            "EMULTIHOP",
            "ENAMETOOLONG",
            "ENETDOWN",
            "ENETRESET",
            "ENETUNREACH",
            "ENFILE",
            "ENOBUFS",
            "ENODATA",
            "ENODEV",
            "ENOENT",
            "ENOEXEC",
            "ENOLCK",
            "ENOLINK",
            "ENOMEM",
            "ENOMSG",
            "ENOPROTOOPT",
            "ENOSPC",
            "ENOSR",
            "ENOSTR",
            "ENOSYS",
            "ENOTCONN",
            "ENOTDIR",
            "ENOTEMPTY",
            "ENOTSOCK",
            "ENOTSUP",
            "ENOTTY",
            "ENXIO",
            "EOPNOTSUPP",
            "EOVERFLOW",
            "EPERM",
            "EPIPE",
            "EPROTO",
            "EPROTONOSUPPORT",
            "EPROTOTYPE",
            "ERANGE",
            "EROFS",
            "ESPIPE",
            "ESRCH",
            "ESTALE",
            "ETIME",
            "ETIMEDOUT",
            "ETXTBSY",
            "EWOULDBLOCK",
            "EXDEV",
            "F_OK",
            "O_APPEND",
            "O_CREAT",
            "O_DIRECTORY",
            "O_DSYNC",
            "O_EXCL",
            "O_NOCTTY",
            "O_NOFOLLOW",
            "O_NONBLOCK",
            "O_RDONLY",
            "O_RDWR",
            "O_SYNC",
            "O_SYMLINK",
            "O_TRUNC",
            "O_WRONLY",
            "R_OK",
            "SIGABRT",
            "SIGALRM",
            "SIGBUS",
            "SIGCHLD",
            "SIGCONT",
            "SIGFPE",
            "SIGHUP",
            "SIGILL",
            "SIGINFO",
            "SIGINT",
            "SIGIO",
            "SIGIOT",
            "SIGKILL",
            "SIGPIPE",
            "SIGPROF",
            "SIGQUIT",
            "SIGSEGV",
            "SIGSTOP",
            "SIGSYS",
            "SIGTERM",
            "SIGTRAP",
            "SIGTSTP",
            "SIGTTIN",
            "SIGTTOU",
            "SIGURG",
            "SIGUSR1",
            "SIGUSR2",
            "SIGVTALRM",
            "SIGWINCH",
            "SIGXCPU",
            "SIGXFSZ",
            "S_IFBLK",
            "S_IFCHR",
            "S_IFDIR",
            "S_IFIFO",
            "S_IFLNK",
            "S_IFMT",
            "S_IFREG",
            "S_IFSOCK",
            "S_IRGRP",
            "S_IROTH",
            "S_IRUSR",
            "S_IRWXG",
            "S_IRWXO",
            "S_IRWXU",
            "S_IWGRP",
            "S_IWOTH",
            "S_IWUSR",
            "S_IXGRP",
            "S_IXOTH",
            "S_IXUSR",
            "W_OK",
            "X_OK",
            "default",
            "signals",
        ],
    },
    V8 {
        variant: V8,
        file: "node/v8.ts",
        logical_path: "node:v8.ts",
        source_const: V8_SOURCE,
        exports_const: V8_EXPORTS,
        sample_source_check: "export function cachedDataVersionTag",
        presence_test: v8_builtin_source_is_present,
        compilation_test: v8_builtin_compiles_cleanly,
        exports: [
            "DefaultDeserializer",
            "DefaultSerializer",
            "Deserializer",
            "GCProfiler",
            "Serializer",
            "cachedDataVersionTag",
            "default",
            "deserialize",
            "getCppHeapStatistics",
            "getHeapCodeStatistics",
            "getHeapSnapshot",
            "getHeapSpaceStatistics",
            "getHeapStatistics",
            "isStringOneByteRepresentation",
            "promiseHooks",
            "queryObjects",
            "serialize",
            "setFlagsFromString",
            "setHeapSnapshotNearHeapLimit",
            "startCpuProfile",
            "startupSnapshot",
            "stopCoverage",
            "takeCoverage",
            "writeHeapSnapshot",
        ],
    },
    Vm {
        variant: Vm,
        file: "node/vm.ts",
        logical_path: "node:vm.ts",
        source_const: VM_SOURCE,
        exports_const: VM_EXPORTS,
        sample_source_check: "export function runInThisContext",
        presence_test: vm_builtin_source_is_present,
        compilation_test: vm_builtin_compiles_cleanly,
        exports: [
            "Script",
            "compileFunction",
            "constants",
            "createContext",
            "createScript",
            "default",
            "isContext",
            "measureMemory",
            "runInContext",
            "runInNewContext",
            "runInThisContext",
        ],
    },
}

/// The Node-mode bootstrap both engines run before user code: the WHATWG web classes, `process`
/// signal events and `process.kill` (realms issue 23), then
/// `Buffer.prototype` over each engine's branded `Uint8Array` (issue 08 of `.scratch/full-runtime`).
pub const WEB_BOOTSTRAP_SOURCE: &str = concat!(
    include_str!("web.ts"),
    "\n",
    // Before `web_events_streams.ts`, which deletes the port primitives this reads.
    include_str!("signals.ts"),
    "\n",
    include_str!("web_events_streams.ts"),
    "\n",
    include_str!("buffer.ts")
);

/// `URL` and `URLSearchParams` over the host's `__kilnUrl*` intrinsics (ADR 0063 rule 4). Part
/// of every native realm's bootstrap, not only `--node`'s: `URL` is an ECMAScript-adjacent global
/// a plain module reaches for.
pub const URL_BOOTSTRAP_SOURCE: &str = include_str!("url.ts");

/// `Date.prototype` over the host's timestamp storage (ADR 0005, ADR 0063 rule 4, Phase 5.1).
/// Part of every realm's bootstrap, in both interpreter and native execution.
pub const DATE_BOOTSTRAP_SOURCE: &str = include_str!("date.ts");

/// `%TypedArray%.prototype` (ADR 0005, ADR 0063 rule 4, Phase 5.1).
pub const TYPEDARRAY_BOOTSTRAP_SOURCE: &str = include_str!("typedarray.ts");
/// The parallel collection family (ADR 0068, ADR 0069): kiln extensions on `Array.prototype`
/// and `%TypedArray%.prototype`, after `typedarray.ts`, whose intrinsics it borrows.
pub const PARALLEL_BOOTSTRAP_SOURCE: &str = include_str!("parallel.ts");
/// Whether the realm installs kiln's own members (ADR 0068); first in the common bootstrap.
pub const EXTENSIONS_BOOTSTRAP_SOURCE: &str = include_str!("extensions.ts");

/// Error subclass prototypes and `SuppressedError` (ADR 0005, ADR 0063 rule 4).
pub const ERRORS_BOOTSTRAP_SOURCE: &str = include_str!("errors.ts");

/// Annex B HTML and Object.prototype extensions and Array.fromAsync (ADR 0005, ADR 0063 rule 4).
pub const ANNEXB_BOOTSTRAP_SOURCE: &str = include_str!("annexb.ts");

/// Locale aware string case methods, backed by the shared Unicode case mappings.
pub const STRING_LOCALE_BOOTSTRAP_SOURCE: &str = include_str!("string_locale.ts");

/// `DisposableStack` and `AsyncDisposableStack` (ES2024 / Stage 3 Explicit Resource Management).
pub const DISPOSABLE_BOOTSTRAP_SOURCE: &str = include_str!("disposable.ts");

/// `RegExp.prototype` accessors and `RegExp.escape` (ECMAScript specification / ES2025).
pub const REGEXP_BOOTSTRAP_SOURCE: &str = include_str!("regexp.ts");

/// `ArrayBuffer.prototype` and `SharedArrayBuffer.prototype` extensions.
pub const ARRAYBUFFER_BOOTSTRAP_SOURCE: &str = include_str!("arraybuffer.ts");

/// FinalizationRegistry's language checks for the native backend. The interpreter already
/// provides this builtin; native installs the TypeScript constructor before arraybuffer.ts
/// captures its native-backend marker.
pub const WEAK_REFS_BOOTSTRAP_SOURCE: &str = include_str!("weak_refs.ts");

/// `Atomics` over the native backend's element primitive and the runtime's waiter list (realms
/// issue 06); installs nothing in the interpreter, whose `Atomics` is its own.
pub const ATOMICS_BOOTSTRAP_SOURCE: &str = include_str!("atomics.ts");

/// `structuredClone` and the structured clone with transfer every message port uses (ADR 0060
/// kind 7): one implementation for both engines, over the detach primitive `arraybuffer.ts`
/// captures. Follows `arraybuffer.ts` in the common bootstrap for that reason.
pub const STRUCTURED_CLONE_BOOTSTRAP_SOURCE: &str = include_str!("structured_clone.ts");

/// `Map.prototype.getOrInsert` and `Map.prototype.getOrInsertComputed` (TC39 Stage 3).
pub const MAPUPSERT_BOOTSTRAP_SOURCE: &str = include_str!("mapupsert.ts");

/// `Promise.try`, `Promise.allKeyed`, and `Promise.allSettledKeyed`.
pub const PROMISE_BOOTSTRAP_SOURCE: &str = include_str!("promise.ts");

/// Node's unhandled-rejection tracking over each engine's `__kilnPromiseRejections` hook.
pub const REJECTIONS_BOOTSTRAP_SOURCE: &str = include_str!("rejections.ts");

/// `Iterator` combinators (`concat`, `zip`, `zipKeyed`) and `Iterator.prototype` methods (`chunks`, `windows`, `join`, `[Symbol.dispose]`).
pub const ITERATOR_BOOTSTRAP_SOURCE: &str = include_str!("iterator.ts");

/// `Object.seal`, `Object.isSealed`, `Reflect.isExtensible`, `Reflect.preventExtensions`, `Reflect.setPrototypeOf`.
pub const OBJECT_REFLECT_BOOTSTRAP_SOURCE: &str = include_str!("object_reflect.ts");

/// Families whose bootstrap state can be omitted from a closed-world native executable when
/// liveness proves that neither the intrinsic nor its prototype can be reached. The interpreter
/// executes the complete bootstrap; native source selection only removes work a program cannot
/// observe. Groups with implicit literal or iterator reachability remain unconditional.
pub mod native_bootstrap_family {
    pub const DATE: u32 = 1 << 0;
    pub const TYPED_ARRAY: u32 = 1 << 1;
    pub const DISPOSABLE: u32 = 1 << 2;
    pub const WEAK_REFS: u32 = 1 << 3;
    pub const ARRAY_BUFFER: u32 = 1 << 4;
    pub const ATOMICS: u32 = 1 << 5;
    pub const STRUCTURED_CLONE: u32 = 1 << 6;
    pub const MAP_UPSERT: u32 = 1 << 7;
    pub const PROMISE: u32 = 1 << 8;
    pub const OBJECT_REFLECT: u32 = 1 << 9;
    pub const STRING_LOCALE: u32 = 1 << 10;
    pub const ALL: u32 = (1 << 11) - 1;
}

/// Keep the script order and lexical dependencies identical to the full bootstrap. An omitted
/// installer must still hide the private host primitive it would have captured and deleted:
/// leaving one on `globalThis` changes both reflection and feature detection.
pub fn common_bootstrap_source_selected(families: u32) -> String {
    use native_bootstrap_family as family;
    let selected = |bit: u32, source: &'static str| {
        if families & bit != 0 { source } else { "" }
    };
    let mut source = [
        EXTENSIONS_BOOTSTRAP_SOURCE,
        selected(family::DATE, DATE_BOOTSTRAP_SOURCE),
        selected(family::TYPED_ARRAY, TYPEDARRAY_BOOTSTRAP_SOURCE),
        PARALLEL_BOOTSTRAP_SOURCE,
        ERRORS_BOOTSTRAP_SOURCE,
        selected(family::STRING_LOCALE, STRING_LOCALE_BOOTSTRAP_SOURCE),
        ANNEXB_BOOTSTRAP_SOURCE,
        selected(family::DISPOSABLE, DISPOSABLE_BOOTSTRAP_SOURCE),
        REGEXP_BOOTSTRAP_SOURCE,
        selected(family::WEAK_REFS, WEAK_REFS_BOOTSTRAP_SOURCE),
        selected(family::ARRAY_BUFFER, ARRAYBUFFER_BOOTSTRAP_SOURCE),
        selected(family::ATOMICS, ATOMICS_BOOTSTRAP_SOURCE),
        selected(family::STRUCTURED_CLONE, STRUCTURED_CLONE_BOOTSTRAP_SOURCE),
        selected(family::MAP_UPSERT, MAPUPSERT_BOOTSTRAP_SOURCE),
        selected(family::PROMISE, PROMISE_BOOTSTRAP_SOURCE),
        REJECTIONS_BOOTSTRAP_SOURCE,
        ITERATOR_BOOTSTRAP_SOURCE,
        selected(family::OBJECT_REFLECT, OBJECT_REFLECT_BOOTSTRAP_SOURCE),
    ]
    .join("\n");
    let cleanup: &[(u32, &str)] = &[
        (family::TYPED_ARRAY, "__kilnTypedArrayOutOfBounds"),
        (family::TYPED_ARRAY, "__kilnTypedArrayCopy"),
        (family::TYPED_ARRAY, "__kilnTypedArraySort"),
        (family::WEAK_REFS, "__kilnSharedWeak"),
        (family::ARRAY_BUFFER, "__kilnArrayBufferDetach"),
        (family::ARRAY_BUFFER, "__kilnArrayBufferResize"),
        (family::ARRAY_BUFFER, "__kilnSharedBuffer"),
        (family::ATOMICS, "__kilnAtomics"),
        (family::STRUCTURED_CLONE, "__kilnCloneData"),
        (family::STRUCTURED_CLONE, "__kilnSharedIs"),
        (family::OBJECT_REFLECT, "__kilnSetPrototypeOf"),
        (family::OBJECT_REFLECT, "__kilnPreventExtensions"),
    ];
    for &(bit, name) in cleanup {
        if families & bit == 0 {
            source.push_str("\ndelete (globalThis as any).");
            source.push_str(name);
            source.push(';');
        }
    }
    source
}

/// Part of every realm's bootstrap, in both interpreter and native execution.
/// Includes Date.prototype, %TypedArray%.prototype, Error subclasses, Annex B, DisposableStack,
/// RegExp.prototype, ArrayBuffer/SharedArrayBuffer, structuredClone, Map.upsert, Promise extensions, unhandled-rejection tracking, Iterator extensions, and Object/Reflect extensions.
pub fn common_bootstrap_source() -> String {
    // The interpreter adapters and native provider both call kiln-intl's ICU4X case mapper.
    // The native backend adds the public TypeScript wrappers only when liveness retains a call.
    common_bootstrap_source_selected(
        native_bootstrap_family::ALL & !native_bootstrap_family::STRING_LOCALE,
    )
}

#[cfg(test)]
mod web_tests {
    use super::*;

    #[test]
    fn common_bootstrap_compiles_as_single_unit() {
        let source = common_bootstrap_source();
        let hir = kiln_frontend_oxc::parse("@kiln/common-bootstrap.ts", &source);
        let program = kiln_ir::compile_module(&hir)
            .expect("common bootstrap should compile to IR as a single unit without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn selected_native_bootstrap_hides_unretained_host_primitives() {
        let source = common_bootstrap_source_selected(0);
        assert!(!source.contains("installDatePrototype"));
        assert!(!source.contains("installTypedArrayPrototype"));
        for name in [
            "__kilnTypedArrayCopy",
            "__kilnArrayBufferDetach",
            "__kilnSharedWeak",
            "__kilnAtomics",
            "__kilnCloneData",
            "__kilnSetPrototypeOf",
        ] {
            assert!(source.contains(&format!("delete (globalThis as any).{name};")));
        }
        let hir = kiln_frontend_oxc::parse("@kiln/selected-bootstrap.ts", &source);
        kiln_ir::compile_module(&hir).expect("selected bootstrap should compile");
    }

    #[test]
    fn regexp_bootstrap_compiles_cleanly() {
        let hir = kiln_frontend_oxc::parse("@kiln/regexp-bootstrap.ts", REGEXP_BOOTSTRAP_SOURCE);
        let program = kiln_ir::compile_module(&hir)
            .expect("regexp bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn arraybuffer_bootstrap_compiles_cleanly() {
        let hir = kiln_frontend_oxc::parse(
            "@kiln/arraybuffer-bootstrap.ts",
            ARRAYBUFFER_BOOTSTRAP_SOURCE,
        );
        let program = kiln_ir::compile_module(&hir)
            .expect("arraybuffer bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn mapupsert_bootstrap_compiles_cleanly() {
        let hir =
            kiln_frontend_oxc::parse("@kiln/mapupsert-bootstrap.ts", MAPUPSERT_BOOTSTRAP_SOURCE);
        let program = kiln_ir::compile_module(&hir)
            .expect("mapupsert bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn promise_bootstrap_compiles_cleanly() {
        let hir = kiln_frontend_oxc::parse("@kiln/promise-bootstrap.ts", PROMISE_BOOTSTRAP_SOURCE);
        let program = kiln_ir::compile_module(&hir)
            .expect("promise bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn iterator_bootstrap_compiles_cleanly() {
        let hir =
            kiln_frontend_oxc::parse("@kiln/iterator-bootstrap.ts", ITERATOR_BOOTSTRAP_SOURCE);
        let program = kiln_ir::compile_module(&hir)
            .expect("iterator bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn object_reflect_bootstrap_compiles_cleanly() {
        let hir = kiln_frontend_oxc::parse(
            "@kiln/object-reflect-bootstrap.ts",
            OBJECT_REFLECT_BOOTSTRAP_SOURCE,
        );
        let program = kiln_ir::compile_module(&hir)
            .expect("object_reflect bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    /// ADR 0068: the bootstrap installs a kiln extension only through its `define*Extension`
    /// helpers, which run only when the realm installs them, and only one the description
    /// declares as the bootstrap's. A member installed with a bare `defineMethod` would survive
    /// `KILN_EXTENSIONS=0`; one installed as an extension without a row would be counted as
    /// "extra (node lacks it)".
    #[test]
    fn bootstrap_extensions_match_the_description() {
        use kiln_builtin_surface::ExtensionInstaller;
        // (owner, the helper that installs on it, the sources that may call it)
        let installers: [(&str, &str, &[&str]); 3] = [
            (
                "%TypedArray%.prototype",
                "defineExtension(\"",
                &[TYPEDARRAY_BOOTSTRAP_SOURCE],
            ),
            (
                "%TypedArray%.prototype",
                "defineTypedArrayExtension(\"",
                &[PARALLEL_BOOTSTRAP_SOURCE],
            ),
            (
                "Array.prototype",
                "defineArrayExtension(\"",
                &[PARALLEL_BOOTSTRAP_SOURCE],
            ),
        ];
        let installed = |owner: &str| -> Vec<String> {
            let mut keys = Vec::new();
            for (o, helper, sources) in &installers {
                if *o != owner {
                    continue;
                }
                for source in *sources {
                    for (index, _) in source.match_indices(helper) {
                        let rest = &source[index + helper.len()..];
                        keys.push(rest[..rest.find('"').expect("a quoted key")].to_owned());
                    }
                }
            }
            keys
        };
        for owner in ["%TypedArray%.prototype", "Array.prototype"] {
            let declared: Vec<&str> = kiln_builtin_surface::KILN_EXTENSIONS
                .iter()
                .filter(|e| e.owner == owner && e.installed_by == ExtensionInstaller::Bootstrap)
                .map(|e| e.key.property_key())
                .collect();
            assert!(!declared.is_empty(), "{owner}");
            let keys = installed(owner);
            for key in &declared {
                assert!(
                    keys.iter().any(|k| k == key),
                    "{owner}.{key} is declared a bootstrap kiln extension but not installed as one"
                );
                assert!(
                    !TYPEDARRAY_BOOTSTRAP_SOURCE.contains(&format!("defineMethod(\"{key}\"")),
                    "{owner}.{key} is installed unconditionally"
                );
            }
            for key in &keys {
                assert!(
                    declared.contains(&key.as_str()),
                    "{owner}.{key} is installed as a kiln extension but not declared"
                );
            }
        }
        // The family's file installs nothing unless the realm installs extensions.
        assert!(PARALLEL_BOOTSTRAP_SOURCE.contains("if (!kilnExtensions) {\n    return;"));
        let common = common_bootstrap_source();
        assert!(common.starts_with(EXTENSIONS_BOOTSTRAP_SOURCE));
    }

    /// ADR 0060 kind 7: a member the description says the bootstrap owns has no engine
    /// implementation, so the bootstrap has to be what defines it -- or it is simply absent.
    #[test]
    fn bootstrap_owned_members_are_defined_by_the_bootstrap() {
        let common = common_bootstrap_source();
        for (global, member) in kiln_builtin_surface::BOOTSTRAP_STATICS {
            assert!(
                common.contains(&format!("defineMethod({global}, \"{member}\"",))
                    || common.contains(&format!("defineProperty({global}, \"{member}\"")),
                "{global}.{member} is declared bootstrap-owned but the common bootstrap does not \
                 define it"
            );
        }
        for row in kiln_builtin_surface::PROTOTYPE_SURFACE {
            if row.binding == kiln_builtin_surface::SemanticBinding::Bootstrap {
                let key = row.key.property_key();
                assert!(
                    common.contains(&format!("\"{key}\"")),
                    "{key} is declared bootstrap-owned but the common bootstrap does not define it"
                );
            }
        }
    }

    /// `parallelReduce`'s tree is part of its meaning (ADR 0069): the leaf the TypeScript
    /// definition folds and the leaf the native lowering folds are one number.
    #[test]
    fn parallel_reduce_leaf_is_one_number() {
        let line = format!(
            "const REDUCE_LEAF = {};",
            kiln_builtin_surface::PARALLEL_REDUCE_LEAF
        );
        assert!(PARALLEL_BOOTSTRAP_SOURCE.contains(&line), "{line}");
    }

    #[test]
    fn parallel_bootstrap_compiles_cleanly() {
        let hir = kiln_frontend_oxc::parse(
            "@kiln/parallel-bootstrap.ts",
            &format!("const kilnExtensions = true;\n{PARALLEL_BOOTSTRAP_SOURCE}"),
        );
        let program = kiln_ir::compile_module(&hir)
            .expect("parallel bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn typedarray_bootstrap_compiles_cleanly() {
        let hir =
            kiln_frontend_oxc::parse("@kiln/typedarray-bootstrap.ts", TYPEDARRAY_BOOTSTRAP_SOURCE);
        let program = kiln_ir::compile_module(&hir)
            .expect("typedarray bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn errors_bootstrap_compiles_cleanly() {
        let hir = kiln_frontend_oxc::parse("@kiln/errors-bootstrap.ts", ERRORS_BOOTSTRAP_SOURCE);
        let program = kiln_ir::compile_module(&hir)
            .expect("errors bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn annexb_bootstrap_compiles_cleanly() {
        let hir = kiln_frontend_oxc::parse("@kiln/annexb-bootstrap.ts", ANNEXB_BOOTSTRAP_SOURCE);
        let program = kiln_ir::compile_module(&hir)
            .expect("annexb bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn string_locale_bootstrap_compiles_cleanly() {
        let hir = kiln_frontend_oxc::parse(
            "@kiln/string-locale-bootstrap.ts",
            STRING_LOCALE_BOOTSTRAP_SOURCE,
        );
        let program = kiln_ir::compile_module(&hir)
            .expect("string locale bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn disposable_bootstrap_compiles_cleanly() {
        let hir =
            kiln_frontend_oxc::parse("@kiln/disposable-bootstrap.ts", DISPOSABLE_BOOTSTRAP_SOURCE);
        let program = kiln_ir::compile_module(&hir)
            .expect("disposable bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn date_bootstrap_compiles_cleanly() {
        let hir = kiln_frontend_oxc::parse("@kiln/date-bootstrap.ts", DATE_BOOTSTRAP_SOURCE);
        let program = kiln_ir::compile_module(&hir)
            .expect("date bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn url_bootstrap_compiles_cleanly() {
        let hir = kiln_frontend_oxc::parse("@kiln/url-bootstrap.ts", URL_BOOTSTRAP_SOURCE);
        let program = kiln_ir::compile_module(&hir)
            .expect("url bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }

    #[test]
    fn web_bootstrap_compiles_cleanly() {
        let hir = kiln_frontend_oxc::parse("@kiln/web-bootstrap.ts", WEB_BOOTSTRAP_SOURCE);
        let program = kiln_ir::compile_module(&hir)
            .expect("web bootstrap should compile to IR without error");
        assert!(!program.program.code.is_empty());
    }
}
