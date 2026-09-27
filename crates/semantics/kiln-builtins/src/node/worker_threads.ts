// kiln-builtins: node:worker_threads (Phase 5.2, ADR 0005; ADR 0070; realms issue 06)
//
// A worker is a realm of its own on a thread of the runtime's realm host, and everything here
// is node's API over three things the realm bootstrap (`web_events_streams.ts`) owns: ports that
// cross realms, the one structured clone every message goes through, and the runtime's worker
// operations (`kiln_runtime::workers`). Each engine only starts the worker's realm and turns
// message tokens into its own values; nothing about node's semantics is written twice.
//
// The worker's channel to its parent carries the program's messages and the runtime's own --
// `online`, the worker's standard output and error, an uncaught exception, `exit` -- in the order
// the worker produced them, so an `exit` is never seen before a message the worker sent first.

import { EventEmitter } from "node:events";

const G = globalThis as any;

// The ports, the clone and the runtime operations, from the bootstrap that owns them. Absent
// outside Node mode, where the realm has no web messaging globals.
const internals: any =
  typeof G.MessagePort === "function" ? G.MessagePort[Symbol.for("kiln.messagePort")] : undefined;

// `kiln_runtime::workers` operation numbers and message kinds (the bootstrap's own list).
const OP_CHANNEL = 1, OP_CLOSE = 4, OP_ENQUEUE = 6, OP_SPAWN = 9, OP_TERMINATE = 10, OP_REAP = 13;
const KIND_USER = 0, KIND_ONLINE = 1, KIND_EXIT = 2, KIND_ERROR = 3, KIND_STDOUT = 4, KIND_STDERR = 5;
const KIND_INIT = 6;

function unavailable(member: string): never {
  throw new Error(
    `node:worker_threads ${member} requires the web messaging globals, which this realm installs ` +
      "only in Node mode"
  );
}

function codedError(Ctor: any, code: string, message: string): Error {
  const error: any = new Ctor(message);
  error.code = code;
  return error;
}

// Node's spelling of what a value is, for `ERR_INVALID_ARG_TYPE`.
function received(value: any): string {
  if (value === null) return "Received null";
  if (value === undefined) return "Received undefined";
  if (typeof value === "function") return `Received function ${value.name}`;
  if (typeof value === "object") {
    const name = value.constructor && value.constructor.name;
    return name ? `Received an instance of ${name}` : "Received an instance of Object";
  }
  let shown = typeof value === "string" ? `'${value.length > 25 ? value.slice(0, 25) + "..." : value}'` : String(value);
  if (typeof value === "bigint") shown += "n";
  return `Received type ${typeof value} (${shown})`;
}

function invalidArgType(name: string, expected: string, value: any): Error {
  return codedError(TypeError, "ERR_INVALID_ARG_TYPE", `The "${name}" ${name.includes(".") ? "property" : "argument"} must be ${expected}. ${received(value)}`);
}

export const MessageChannel = typeof G.MessageChannel === "function"
  ? G.MessageChannel
  : class MessageChannel {
      constructor() {
        unavailable("MessageChannel");
      }
    };

export const MessagePort = typeof G.MessagePort === "function"
  ? G.MessagePort
  : class MessagePort {
      constructor() {
        unavailable("MessagePort");
      }
    };

export const BroadcastChannel = typeof G.BroadcastChannel === "function"
  ? G.BroadcastChannel
  : class BroadcastChannel {
      constructor(_name: string) {
        unavailable("BroadcastChannel");
      }
    };

export const SHARE_ENV = Symbol.for("nodejs.worker_threads.SHARE_ENV");

// What this realm is: a worker's state from the bootstrap, or `undefined` on the main thread.
const self: any = internals !== undefined ? internals.worker() : undefined;

export const isMainThread = self === undefined;
export const isInternalThread = false;
export const parentPort = self === undefined ? null : self.parentPort;
export const threadId: number = self === undefined ? 0 : self.threadId;
export const threadName: string = self === undefined ? "" : self.info.name;
export const workerData = self === undefined ? null : self.workerData;
export const resourceLimits: any = self === undefined ? {} : { ...self.info.resourceLimits };

const envData = new Map<any, any>();
if (self !== undefined) {
  for (const [key, value] of self.environmentData) envData.set(key, value);
}

export function getEnvironmentData(key: any): any {
  return envData.get(key);
}

export function setEnvironmentData(key: any, value: any): void {
  if (value === undefined) envData.delete(key);
  else envData.set(key, value);
}

// Node's defaults for a worker's heap and stack, reported for the limits a program did not set.
// A realm here has no separate young and old generations and no code range to size, so these
// are what node would use, not limits this engine enforces; `stackSizeMb` is what the worker's
// thread is given at most.
const DEFAULT_LIMITS: Record<string, number> = {
  maxYoungGenerationSizeMb: 96,
  maxOldGenerationSizeMb: 2048,
  codeRangeSizeMb: 0,
  stackSizeMb: 4,
};

// The limits a worker runs with (node's defaults for those not given), and what its parent's
// `worker.resourceLimits` reports: the same when none were given, and otherwise the given ones
// with -1 for the rest -- node's parent reports the configuration it passed until the worker
// reads it back.
function limitsOf(options: any): [Record<string, number>, Record<string, number>] {
  const out: Record<string, number> = { ...DEFAULT_LIMITS };
  const given = options.resourceLimits;
  if (given === undefined) return [out, { ...out }];
  if (given === null || typeof given !== "object") throw invalidArgType("options.resourceLimits", "of type object", given);
  const reported: Record<string, number> = {};
  for (const key of Object.keys(DEFAULT_LIMITS)) {
    const value = given[key];
    if (typeof value === "number" && value > 0) {
      out[key] = value;
      reported[key] = value;
    } else {
      reported[key] = key === "stackSizeMb" ? DEFAULT_LIMITS.stackSizeMb : -1;
    }
  }
  return [out, reported];
}

function stringList(value: any, name: string): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value)) throw invalidArgType(name, "an instance of Array", value);
  return value.map((item: any) => `${item}`);
}

// Node's rule for a worker's script: a path that is absolute or explicitly relative (resolved
// against the working directory), or a `file:` URL.
function scriptPath(filename: any): string {
  if (typeof filename === "string") {
    if (filename.startsWith("file:")) return scriptPath(new URL(filename));
    if (filename.startsWith("/") || filename.startsWith("./") || filename.startsWith("../") ||
        (filename.length > 2 && filename[1] === ":" && (filename[2] === "\\" || filename[2] === "/"))) {
      return resolvePath(filename);
    }
    throw codedError(TypeError, "ERR_WORKER_PATH",
      "The worker script or module filename must be an absolute path or a relative path starting with './' or '../'. " +
      `Received "${filename}"`);
  }
  if (filename !== null && typeof filename === "object" && typeof filename.href === "string" && typeof filename.protocol === "string") {
    if (filename.protocol !== "file:") throw codedError(TypeError, "ERR_INVALID_URL_SCHEME", "The URL must be of scheme file");
    return decodeURIComponent(filename.pathname);
  }
  throw invalidArgType("filename", "of type string or an instance of URL", filename);
}

function resolvePath(path: string): string {
  if (path.startsWith("/")) return normalize(path);
  const cwd = G.process && typeof G.process.cwd === "function" ? G.process.cwd() : "/";
  return normalize(cwd + "/" + path);
}

function normalize(path: string): string {
  const parts: string[] = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return "/" + parts.join("/");
}

// The worker's standard streams, when the parent asked for them (`stdout: true`): readable
// streams this side pushes the worker's writes into. Otherwise they go straight to this
// process's own streams, as node pipes them.
function streamOf(): any {
  const stream = typeof G.process?.getBuiltinModule === "function" ? G.process.getBuiltinModule("node:stream") : undefined;
  if (stream && typeof stream.Readable === "function") return new stream.Readable({ read() {} });
  return null;
}

export class Worker extends EventEmitter {
  #port: any;
  #control = 0;
  #threadId: number;
  // The runtime's identity for this worker, which `terminate` names.
  #key = 0;
  #exitCode: number | undefined = undefined;
  #exited = false;
  #limits: Record<string, number>;
  #waiting: ((code: number | undefined) => void)[] = [];
  #stdout: any = null;
  #stderr: any = null;
  #pipeStdout: boolean;
  #pipeStderr: boolean;

  constructor(filename: any, options?: any) {
    super();
    if (internals === undefined) unavailable("Worker");
    if (options === undefined || options === null) options = {};
    else if (typeof options !== "object") throw invalidArgType("options", "of type object", options);
    const isEval = !!options.eval;
    let entry: string;
    if (isEval) {
      if (typeof filename !== "string") {
        throw invalidArgType("filename", "of type string", filename);
      }
      entry = filename;
    } else {
      entry = scriptPath(filename);
    }
    const argv = stringList(options.argv, "options.argv");
    const execArgv = stringList(options.execArgv, "options.execArgv");
    void execArgv;
    const [limits, reported] = limitsOf(options);
    const name = options.name === undefined ? "" : `${options.name}`;
    // A copy of this process's environment when it is made, unless the worker is given one, or
    // shares this one: `SHARE_ENV` here means the process environment the realm reads, which
    // every realm in the process already shares.
    let env: any = null;
    if (options.env === undefined) {
      env = G.process && G.process.env ? { ...G.process.env } : null;
    } else if (options.env !== SHARE_ENV) {
      if (options.env === null || typeof options.env !== "object") throw invalidArgType("options.env", "of type object", options.env);
      env = {};
      for (const key of Object.keys(options.env)) env[key] = `${options.env[key]}`;
    }
    let transferList: any[] | undefined;
    if (options.transferList !== undefined) {
      if (!Array.isArray(options.transferList)) throw invalidArgType("options.transferList", "an instance of Array", options.transferList);
      transferList = options.transferList;
    }
    const environmentData: any[] = [];
    for (const entryPair of envData) environmentData.push(entryPair);
    const start = internals.encodeMessage([options.workerData, env, environmentData], transferList, null);

    const channel = internals.workerOp(OP_CHANNEL);
    const control = internals.workerOp(OP_CHANNEL);
    internals.workerOp(OP_ENQUEUE, channel[1], KIND_INIT, start);
    this.#pipeStdout = !options.stdout;
    this.#pipeStderr = !options.stderr;
    if (!this.#pipeStdout) this.#stdout = streamOf();
    if (!this.#pipeStderr) this.#stderr = streamOf();
    const info = JSON.stringify({ name, resourceLimits: limits, eval: isEval, filename: isEval ? "[worker eval]" : entry });
    const execPath = G.process && typeof G.process.execPath === "string" ? G.process.execPath
      : G.process && Array.isArray(G.process.argv) && G.process.argv.length > 0 ? G.process.argv[0] : "kiln";
    const processArgv = [execPath, isEval ? "[worker eval]" : entry, ...argv];
    let started: number[];
    try {
      started = internals.workerOp(OP_SPAWN, entry, isEval, processArgv, channel[1], control[1], info);
    } catch (error) {
      for (const endpoint of [channel[0], channel[1], control[0], control[1]]) internals.workerOp(OP_CLOSE, endpoint);
      throw error;
    }
    this.#key = started[0];
    this.#threadId = started[1];
    this.#control = control[0];
    this.#limits = reported;
    this.#port = internals.internalPort(channel[0], (kind: number, value: any) => this.#receive(kind, value), null, false);
    // A running worker keeps its parent alive until it exits or is unreferenced.
    this.#port.start();
  }

  #receive(kind: number, value: any): void {
    switch (kind) {
      case KIND_USER:
        this.emit("message", value);
        return;
      case KIND_ONLINE:
        this.emit("online");
        return;
      case KIND_STDOUT:
      case KIND_STDERR: {
        const toErr = kind === KIND_STDERR;
        const stream = toErr ? this.#stderr : this.#stdout;
        if (toErr ? this.#pipeStderr : this.#pipeStdout) {
          const target = toErr ? G.process?.stderr : G.process?.stdout;
          if (target && typeof target.write === "function") target.write(value);
        } else if (stream !== null) {
          stream.push(value);
        }
        return;
      }
      case KIND_ERROR:
        this.emit("error", value);
        return;
      case KIND_EXIT: {
        const code = typeof value === "number" ? value : 1;
        this.#exited = true;
        this.#exitCode = code;
        this.#port.close();
        internals.workerOp(OP_CLOSE, this.#control);
        internals.workerOp(OP_REAP, this.#key);
        if (this.#stdout !== null) this.#stdout.push(null);
        if (this.#stderr !== null) this.#stderr.push(null);
        const waiting = this.#waiting;
        this.#waiting = [];
        this.emit("exit", code);
        for (const resolve of waiting) resolve(code);
        return;
      }
      case -1:
        this.emit("messageerror", value);
        return;
    }
  }

  postMessage(value: any, transferList?: any): void {
    if (value === undefined && arguments.length === 0) {
      throw codedError(TypeError, "ERR_MISSING_ARGS", 'The "value" argument must be specified');
    }
    if (this.#exited) return;
    this.#port.postMessage(value, transferList);
  }

  // Resolves with the exit code once the worker has stopped; `undefined` for one that already
  // had.
  terminate(callback?: any): Promise<number | undefined> {
    if (this.#exited) {
      if (typeof callback === "function") G.process.nextTick(callback, null, undefined);
      return Promise.resolve(undefined);
    }
    const done = new Promise<number | undefined>((resolve) => this.#waiting.push(resolve));
    if (typeof callback === "function") done.then((code) => callback(null, code));
    internals.workerOp(OP_TERMINATE, this.#key);
    return done;
  }

  ref(): void {
    if (!this.#exited) this.#port.ref();
  }

  unref(): void {
    if (!this.#exited) this.#port.unref();
  }

  get threadId(): number {
    return this.#exited ? -1 : this.#threadId;
  }

  get resourceLimits(): any {
    return this.#exited ? {} : { ...this.#limits };
  }

  get stdout(): any {
    return this.#stdout;
  }

  get stderr(): any {
    return this.#stderr;
  }

  get stdin(): any {
    return null;
  }

  // Node measures the worker's event loop from the parent; no realm here keeps those counters,
  // and zeros would read as an idle worker.
  get performance(): any {
    return {
      eventLoopUtilization(): never {
        throw new Error("node:worker_threads Worker.performance.eventLoopUtilization is not supported by kiln yet");
      },
    };
  }

  getHeapSnapshot(): never {
    throw new Error("node:worker_threads Worker.getHeapSnapshot is not supported by kiln yet");
  }

  [Symbol.asyncDispose](): Promise<number | undefined> {
    return this.terminate();
  }
}

// The structured clone's own marks (crates/kiln-builtins/src/structured_clone.ts), reached through
// the symbol the bootstrap hangs them on: this module is not part of that script.
const cloneInternals: any = (globalThis as any).structuredClone?.[Symbol.for("kiln.structuredClone")];

/// A marked object in a transfer list is a DataCloneError; cloned, it is still copied.
export function markAsUntransferable(obj: any): void {
  if (cloneInternals) cloneInternals.markAsUntransferable(obj);
}
export function isMarkedAsUntransferable(obj: any): boolean {
  return cloneInternals ? cloneInternals.isMarkedAsUntransferable(obj) : false;
}

// Node makes a marked object throw when it is cloned. The structured clone keeps no uncloneable
// marks, so accepting one would clone what Node refuses to; refuse the mark instead.
export function markAsUncloneable(_obj: any): void {
  throw new Error("node:worker_threads markAsUncloneable is not supported by kiln yet");
}

// Moving a port into another `vm` context needs per-context ports, which no realm has.
export function moveMessagePortToContext(_port: any, _context: any): any {
  throw new Error("node:worker_threads moveMessagePortToContext is not supported by kiln yet");
}

// Node's thread-addressed messages arrive as a `workerMessage` event on the receiving thread's
// `process`, which no realm here emits yet.
export function postMessageToThread(_threadId: any, _value: any, _transferList?: any, _timeout?: any): Promise<void> {
  return Promise.reject(new Error("node:worker_threads postMessageToThread is not supported by kiln yet"));
}

// Take the oldest message a port has not yet dispatched, wrapped, as Node does, whether or not
// the port has been started -- from this realm's queue or, for a port whose other end is in
// another realm, from the runtime's. The port's queue is private to `MessagePort`
// (web_events_streams.ts), which hands this module its `receive` through the same kind of
// symbol-keyed internals the structured clone uses.
export function receiveMessageOnPort(port: any): any {
  if (internals === undefined || !(port instanceof MessagePort)) {
    const error: any = new TypeError('The "port" argument must be a MessagePort instance');
    error.code = "ERR_INVALID_ARG_TYPE";
    throw error;
  }
  return internals.receive(port);
}

const workerThreadsModule = {
  BroadcastChannel,
  MessageChannel,
  MessagePort,
  SHARE_ENV,
  Worker,
  default: undefined as any,
  getEnvironmentData,
  isInternalThread,
  isMainThread,
  markAsUncloneable,
  markAsUntransferable,
  isMarkedAsUntransferable,
  moveMessagePortToContext,
  parentPort,
  postMessageToThread,
  receiveMessageOnPort,
  resourceLimits,
  setEnvironmentData,
  threadId,
  threadName,
  workerData,
};
workerThreadsModule.default = workerThreadsModule;

export default workerThreadsModule;
