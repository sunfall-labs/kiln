// kiln-builtins: node:module implementation (Phase 5.2, ADR 0005)

import { URL as NodeURL } from "node:url";

export const builtinModules: string[] = [
  "assert",
  "assert/strict",
  "async_hooks",
  "buffer",
  "child_process",
  "cluster",
  "console",
  "constants",
  "crypto",
  "dgram",
  "diagnostics_channel",
  "dns",
  "domain",
  "events",
  "fs",
  "fs/promises",
  "http",
  "http2",
  "https",
  "inspector",
  "module",
  "net",
  "os",
  "path",
  "perf_hooks",
  "process",
  "punycode",
  "querystring",
  "readline",
  "repl",
  "stream",
  "stream/consumers",
  "stream/promises",
  "stream/web",
  "string_decoder",
  "timers",
  "timers/promises",
  "tls",
  "trace_events",
  "tty",
  "url",
  "util",
  "util/types",
  "v8",
  "vm",
  "wasi",
  "worker_threads",
  "zlib",
];

export function isBuiltin(moduleName: string): boolean {
  if (typeof moduleName !== "string") return false;
  const name = moduleName.startsWith("node:") ? moduleName.slice(5) : moduleName;
  return builtinModules.indexOf(name) !== -1;
}

export const constants = {
  compileCacheStatus: {
    FAILED: 0,
    ENABLED: 1,
    ALREADY_ENABLED: 2,
    DISABLED: 3,
  },
};

// Capture the host capability while the builtin module is initialized so later writes to the
// public process object cannot redirect createRequire.
const hostCreateRequire = (globalThis as any).__kilnPrivate.process?.__kilnNativeModules
  ?.__kilnHostModuleCreateRequire;

// In a kiln native binary every module is ahead-of-time compiled, so the
// compile cache is permanently "already enabled" — there is nothing to warm
// up and no cache directory. This is a semantic no-op, not a stub.
export function enableCompileCache(_cacheDir?: string): { status: number } {
  return { status: constants.compileCacheStatus.ALREADY_ENABLED };
}

export function getCompileCacheDir(): undefined {
  return undefined;
}

export function flushCompileCache(): void {}

export function createRequire(filename: string | any): (specifier: string) => any {
  const isUrlObject =
    filename instanceof NodeURL ||
    (filename !== null && typeof filename === "object" && typeof filename.href === "string");
  const isAbsoluteFilename =
    typeof filename === "string" &&
    (filename.startsWith("/") || /^[A-Za-z]:[\\/]/.test(filename));
  const isFileUrlObject =
    isUrlObject &&
    (filename.protocol === "file:" ||
      (typeof filename.href === "string" && filename.href.startsWith("file://")));
  const isFileUrlString = typeof filename === "string" && filename.startsWith("file://");
  if (typeof filename !== "string" && !isFileUrlObject ||
      (typeof filename === "string" && !isAbsoluteFilename && !isFileUrlString)) {
    throw new TypeError("The argument must be an absolute path or a file URL");
  }
  // The host captures this immutable filename and binds the returned callable to the
  // graph loader for that importer. Keeping the hook private prevents a process-global
  // require from bypassing graph identity and cache ownership.
  return hostCreateRequire(
    typeof filename === "string" ? filename : filename.href,
  );
}

export class Module {
  static Module: typeof Module = Module;
  static default: typeof Module;
  static createRequire: typeof createRequire = createRequire;
  static builtinModules: string[] = builtinModules;
  static isBuiltin: typeof isBuiltin = isBuiltin;
  static constants: typeof constants = constants;
  static enableCompileCache: typeof enableCompileCache = enableCompileCache;
  static getCompileCacheDir: typeof getCompileCacheDir = getCompileCacheDir;
  static flushCompileCache: typeof flushCompileCache = flushCompileCache;
  static _cache: any = Object.create(null);
  static _extensions: any = Object.create(null);

  id: string;
  filename: string;
  loaded: boolean = false;
  exports: any;
  parent: any;
  children: any[] = [];
  paths: string[] = [];

  constructor(id: string = "", parent?: any) {
    this.id = id;
    this.filename = id;
    this.parent = parent || null;
    this.exports = {};
  }

  require(id: string): any {
    return createRequire(this.filename)(id);
  }
}

Module.Module = Module;
Module.default = Module;

export default Module;
