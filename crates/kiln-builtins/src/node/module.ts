// kiln-builtins: node:module implementation (Phase 5.2, ADR 0005)

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

export function createRequire(_filename: string | any): (specifier: string) => any {
  const globalRequire = (globalThis as any).require;
  if (typeof globalRequire === "function") {
    return globalRequire;
  }
  return (specifier: string) => {
    throw new Error(`Cannot find module '${specifier}'`);
  };
}

export class Module {
  static Module: typeof Module = Module;
  static default: typeof Module;
  static createRequire: typeof createRequire = createRequire;
  static builtinModules: string[] = builtinModules;
  static isBuiltin: typeof isBuiltin = isBuiltin;
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
