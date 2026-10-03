// kiln-builtins: node:child_process implementation (Phase 5.2, ADR 0005)

import { EventEmitter } from "node:events";
import { Buffer } from "node:buffer";

const nativeModules =
  typeof process !== "undefined" ? (process as any).__kilnNativeModules : undefined;
const nativeModule = nativeModules ? nativeModules.child_process || null : null;

export class ChildProcess extends EventEmitter {
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

  kill(signal?: string | number): boolean {
    if (this.killed) return false;
    this.killed = true;
    if (nativeModule && typeof nativeModule.kill === "function" && this.pid) {
      return nativeModule.kill(this.pid, signal);
    }
    return true;
  }

  ref(): this {
    return this;
  }

  unref(): this {
    return this;
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
  const child = new ChildProcess();
  child.pid = 0;
  return child;
}

export function spawnSync(command: string, args?: any, options?: any): any {
  if (typeof args === "string" || args == null) {
    options = args ?? options;
    args = [];
  }
  if (!Array.isArray(args)) throw new TypeError("spawnSync arguments must be an array")
  const settings = normalizeSyncOptions(options)
  const raw = runFileSync(command, args)
  const stdout = Buffer.from(raw.stdout, "latin1")
  const stderr = Buffer.from(raw.stderr, "latin1")
  const encoding = settings?.encoding
  const visibleOut = encoding ? stdout.toString(encoding) : stdout
  const visibleErr = encoding ? stderr.toString(encoding) : stderr
  return {
    pid: raw.error ? undefined : 0,
    output: [null, visibleOut, visibleErr],
    stdout: visibleOut,
    stderr: visibleErr,
    status: raw.status < 0 ? null : raw.status,
    signal: raw.status < 0 ? "SIGTERM" : null,
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
  const child = new ChildProcess();
  if (typeof callback === "function") {
    queueMicrotask(() => {
      callback(null, "", "");
    });
  }
  return child;
}

export function execSync(command: string, options?: any): any {
  if (nativeModule && typeof nativeModule.execSync === "function") {
    return nativeModule.execSync(command, options);
  }
  const result = spawnSync(command, [], { ...options, shell: true });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`Command failed: ${command}`);
  return result.stdout;
}

export function execFile(file: string, args?: any, options?: any, callback?: any): ChildProcess {
  if (typeof args === "function") {
    callback = args;
    args = [];
    options = undefined;
  } else if (typeof options === "function") {
    callback = options;
    options = undefined;
  }
  if (nativeModule && typeof nativeModule.execFile === "function") {
    return nativeModule.execFile(file, args, options, callback);
  }
  const child = new ChildProcess();
  if (typeof callback === "function") {
    queueMicrotask(() => {
      callback(null, "", "");
    });
  }
  return child;
}

export function execFileSync(file: string, args?: any, options?: any): any {
  if (typeof args === "string" || args == null) {
    options = args ?? options;
    args = [];
  }
  if (!Array.isArray(args)) throw new TypeError("execFileSync arguments must be an array")
  const settings = normalizeSyncOptions(options)
  const raw = runFileSync(file, args)
  const stdout = Buffer.from(raw.stdout, "latin1")
  const stderr = Buffer.from(raw.stderr, "latin1")
  if (raw.error || raw.status !== 0) {
    const error = new Error(raw.error || `Command failed: ${file}`) as Error & {
      status?: number | null
      stdout?: any
      stderr?: any
    }
    error.status = raw.status < 0 ? null : raw.status
    error.stdout = stdout
    error.stderr = stderr
    throw error
  }
  return settings?.encoding ? stdout.toString(settings.encoding) : stdout
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
  status: number
  error: string | null
} {
  if (nativeModule && typeof nativeModule.__execFileSync === "function") {
    return nativeModule.__execFileSync(String(file), ...args.map((argument) => String(argument)))
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
