// kiln-builtins: node:child_process implementation (Phase 5.2, ADR 0005)

import { EventEmitter } from "node:events";
import { Buffer } from "node:buffer";
import { signals } from "node:constants";

const nativeModules =
  typeof process !== "undefined" ? (process as any).__kilnNativeModules : undefined;
const nativeModule = nativeModules ? nativeModules.child_process || null : null;
const enginePrivate: any = (globalThis as any).__kilnPrivate;

export class ChildProcess extends EventEmitter {
  /** Set when ProcessRun returns; this host operation does not expose the pid at start time. */
  pid?: number;
  stdin: any = null;
  stdout: any = null;
  stderr: any = null;
  stdio: any[] = [null, null, null];
  killed: boolean = false;
  exitCode: number | null = null;
  signalCode: string | null = null;
  connected: boolean = false;

  constructor() {
    super();
  }

  kill(_signal?: string | number): boolean {
    throw new Error("node:child_process kill is unavailable: this host operation has no child handle");
  }

  ref(): this {
    throw new Error("ChildProcess.ref is unavailable for this host operation");
  }

  unref(): this {
    throw new Error("ChildProcess.unref is unavailable for this host operation");
  }

  disconnect(): void {}

  send(_message: any, ..._args: any[]): boolean {
    return false;
  }
}

export function spawn(command: string, args?: any, options?: any): ChildProcess {
  if (nativeModule && typeof nativeModule.spawn === "function") {
    return nativeModule.spawn(command, args, options);
  }
  throw new Error("node:child_process spawn is unavailable in this runtime");
}

export function spawnSync(command: string, args?: any, options?: any): any {
  if (nativeModule && typeof nativeModule.spawnSync === "function") {
    return nativeModule.spawnSync(command, args, options);
  }
  if (args == null) {
    args = [];
  } else if (typeof args === "object" && !Array.isArray(args) && options === undefined) {
    options = args;
    args = [];
  }
  if (!Array.isArray(args)) throw new TypeError("spawnSync arguments must be an array")
  if (typeof command !== "string") throw new TypeError("spawnSync command must be a string")
  const settings = normalizeSyncOptions(options)
  const raw = runFileSync(command, args)
  const stdout = Buffer.from(raw.stdout, "latin1")
  const stderr = Buffer.from(raw.stderr, "latin1")
  const encoding = settings?.encoding
  const visibleOut = encoding ? stdout.toString(encoding) : stdout
  const visibleErr = encoding ? stderr.toString(encoding) : stderr
  return {
    pid: raw.pid,
    output: [null, visibleOut, visibleErr],
    stdout: visibleOut,
    stderr: visibleErr,
    status: raw.status,
    signal: raw.signal,
    error: raw.error ? new Error(raw.error) : undefined,
  }
}

export function exec(command: string, options?: any, callback?: any): ChildProcess {
  if (typeof options === "function") {
    callback = options;
    options = undefined;
  }
  if (nativeModule && typeof nativeModule.exec === "function") {
    return nativeModule.exec(command, options, callback);
  }
  throw new Error("node:child_process exec is unavailable in this runtime");
}

export function execSync(command: string, options?: any): any {
  if (nativeModule && typeof nativeModule.execSync === "function") {
    return nativeModule.execSync(command, options);
  }
  throw new Error("node:child_process execSync is unavailable in this runtime");
}

export function execFile(file: string, args?: any, options?: any, callback?: any): ChildProcess {
  if (typeof args === "function") {
    callback = args;
    args = [];
    options = undefined;
  } else if (!Array.isArray(args) && args !== undefined && args !== null) {
    if (typeof options === "function") {
      callback = options;
      options = args;
      args = [];
    } else if (options === undefined) {
      options = args;
      args = [];
    }
  } else if (typeof options === "function") {
    callback = options;
    options = undefined;
  }
  if (nativeModule && typeof nativeModule.execFile === "function") {
    return nativeModule.execFile(file, args, options, callback);
  }
  if (args === undefined || args === null) args = [];
  if (!Array.isArray(args)) throw new TypeError("execFile arguments must be an array");
  if (typeof file !== "string") throw new TypeError("execFile file must be a string");
  const settings = normalizeExecFileOptions(options);
  const strings = args.map((argument: any) => String(argument));
  const task = startHostProcess(file, strings);
  const child = new ChildProcess();
  // ProcessRun reports its identity with the completed output. Until then pid is undefined.
  if (typeof callback === "function") child.on("error", () => {});
  void task.promise.then((result) => {
    child.pid = result.pid;
    child.exitCode = result.exitCode;
    child.signalCode = result.signal === null ? null : signalName(result.signal);
    const stdout = outputValue(result.stdout, settings.encoding);
    const stderr = outputValue(result.stderr, settings.encoding);
    let error: any = null;
    if (result.exitCode !== 0 || result.signal !== null) {
      error = new Error("Command failed: " + file + " " + strings.join(" "));
      error.code = result.exitCode;
      error.signal = child.signalCode;
      error.cmd = file + " " + strings.join(" ");
      error.stdout = stdout;
      error.stderr = stderr;
    }
    child.emit("exit", result.exitCode, child.signalCode);
    try {
      if (typeof callback === "function") callback(error, stdout, stderr);
    } finally {
      child.emit("close", result.exitCode, child.signalCode);
    }
  }, (error) => {
    // A spawn failure has no exit event. Node emits the callback before error/close,
    // and its negative OS errno is also the close status. Preserve the host metadata.
    child.exitCode = null;
    const stdout = outputValue(new Uint8Array(0), settings.encoding);
    const stderr = outputValue(new Uint8Array(0), settings.encoding);
    try {
      if (typeof callback === "function") callback(error, stdout, stderr);
    } finally {
      try {
        child.emit("error", error);
      } finally {
        child.emit("close", typeof error.errno === "number" ? error.errno : null, null);
      }
    }
  });
  return child;
}

function normalizeExecFileOptions(options: any): { encoding: string | null } {
  if (options === undefined || options === null) return { encoding: "utf8" };
  const settings = typeof options === "string" ? { encoding: options } : options;
  if (typeof settings !== "object" || Array.isArray(settings)) {
    throw new TypeError("execFile options must be a string or object");
  }
  for (const key of Object.keys(settings)) {
    if (key !== "encoding") throw new Error("node:child_process execFile option is not supported: " + key);
  }
  const encoding = settings.encoding;
  if (encoding === undefined || encoding === "utf8" || encoding === "utf-8") return { encoding: "utf8" };
  if (encoding === "buffer" || encoding === null) return { encoding: null };
  throw new Error("node:child_process execFile encoding is not supported: " + String(encoding));
}

function outputValue(bytes: Uint8Array, encoding: string | null): any {
  const buffer = Buffer.from(bytes);
  return encoding === null ? buffer : buffer.toString(encoding);
}

function signalName(signal: number): string {
  for (const name of Object.keys(signals)) {
    if (signals[name] === signal) return name;
  }
  throw new Error("node:child_process cannot identify host signal " + signal);
}

function startHostProcess(command: string, args: string[]): { promise: Promise<any> } {
  const start = enginePrivate?.processRun;
  if (typeof start !== "function") throw new Error("asynchronous child process execution is unavailable in this runtime");
  const pending = start(command, args);
  return {
    promise: pending.promise,
  };
}

export function execFileSync(file: string, args?: any, options?: any): any {
  if (nativeModule && typeof nativeModule.execFileSync === "function") {
    return nativeModule.execFileSync(file, args, options);
  }
  if (args == null) {
    args = [];
  } else if (typeof args === "object" && !Array.isArray(args) && options === undefined) {
    options = args;
    args = [];
  }
  if (!Array.isArray(args)) throw new TypeError("execFileSync arguments must be an array")
  if (typeof file !== "string") throw new TypeError("execFileSync file must be a string")
  const settings = normalizeSyncOptions(options)
  const raw = runFileSync(file, args)
  const stdout = Buffer.from(raw.stdout, "latin1")
  const stderr = Buffer.from(raw.stderr, "latin1")
  const visibleOut = settings?.encoding ? stdout.toString(settings.encoding) : stdout
  const visibleErr = settings?.encoding ? stderr.toString(settings.encoding) : stderr
  if (raw.error || raw.status !== 0) {
    const error = new Error(raw.error || `Command failed: ${file}`) as Error & {
      status?: number | null
      signal?: string | null
      stdout?: any
      stderr?: any
    }
    error.status = raw.status
    error.signal = raw.signal
    error.stdout = visibleOut
    error.stderr = visibleErr
    throw error
  }
  return visibleOut
}

function normalizeSyncOptions(options: any): { encoding?: string } | undefined {
  if (options == null) return undefined
  const settings = typeof options === "string" ? { encoding: options } : options
  if (typeof settings !== "object") throw new TypeError("child process options must be a string or object")
  for (const key of Object.keys(settings)) {
    if (key !== "encoding") throw new Error(`node:child_process option is not supported: ${key}`)
  }
  const encoding = settings.encoding
  if (encoding !== undefined && encoding !== "utf8" && encoding !== "utf-8") {
    throw new Error(`node:child_process encoding is not supported: ${String(encoding)}`)
  }
  return encoding === undefined ? undefined : { encoding }
}

function runFileSync(file: string, args: ReadonlyArray<any>): {
  stdout: string
  stderr: string
  status: number | null
  pid: number | undefined
  signal: string | null
  error: string | null
} {
  if (nativeModule && typeof nativeModule.__execFileSync === "function") {
    return nativeModule.__execFileSync(file, ...args.map((argument) => String(argument)))
  }
  throw new Error("node:child_process synchronous execution is unavailable in this runtime")
}

export function fork(modulePath: string, args?: any, options?: any): ChildProcess {
  if (nativeModule && typeof nativeModule.fork === "function") {
    return nativeModule.fork(modulePath, args, options);
  }
  return spawn(process.execPath || "kiln", [modulePath, ...(args || [])], options);
}

const childProcessNamespace = {
  ChildProcess,
  default: undefined as any,
  exec,
  execFile,
  execFileSync,
  execSync,
  fork,
  spawn,
  spawnSync,
};
childProcessNamespace.default = childProcessNamespace;

export default childProcessNamespace;
