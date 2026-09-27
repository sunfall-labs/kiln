// kiln-builtins: node:worker_threads implementation (Phase 5.2, ADR 0005)

export class Worker {
  constructor(_filename: string | any, _options?: any) {
    throw new Error("node:worker_threads requires an isolated realm worker capability");
  }
}

// Messaging is the web platform's in-realm `MessageChannel`, `MessagePort` and `BroadcastChannel`
// (`web_events_streams.ts`), which the realm installs in Node mode. Outside Node mode there are no
// such globals, and a stand-in that accepted messages and never delivered them would be a silent
// wrong answer; each stand-in refuses by name instead (ADR 0063). Cross-realm workers are ADR 0070.
function unavailable(member: string): never {
  throw new Error(
    `node:worker_threads ${member} requires the web messaging globals, which this realm installs ` +
      "only in Node mode"
  );
}

const G = globalThis as any;

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

export const isMainThread = true;
export const parentPort = null;
export const threadId = 0;
export const workerData = null;
export const resourceLimits = {};

const envData = new Map<any, any>();

export function getEnvironmentData(key: any): any {
  return envData.get(key);
}

export function setEnvironmentData(key: any, value: any): void {
  envData.set(key, value);
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

// Take the oldest message a port has not yet dispatched, wrapped, as Node does, whether or not
// the port has been started. The port's queue is private to `MessagePort`
// (web_events_streams.ts), which hands this module its `receive` through the same kind of
// symbol-keyed internals the structured clone uses.
const portInternals: any =
  typeof MessagePort === "function" ? (MessagePort as any)[Symbol.for("kiln.messagePort")] : undefined;

export function receiveMessageOnPort(port: any): any {
  if (portInternals === undefined || !(port instanceof MessagePort)) {
    const error: any = new TypeError('The "port" argument must be a MessagePort instance');
    error.code = "ERR_INVALID_ARG_TYPE";
    throw error;
  }
  return portInternals.receive(port);
}

const workerThreadsModule = {
  BroadcastChannel,
  MessageChannel,
  MessagePort,
  SHARE_ENV,
  Worker,
  default: undefined as any,
  getEnvironmentData,
  isMainThread,
  markAsUncloneable,
  markAsUntransferable,
  isMarkedAsUntransferable,
  moveMessagePortToContext,
  parentPort,
  receiveMessageOnPort,
  resourceLimits,
  setEnvironmentData,
  threadId,
  workerData,
};
workerThreadsModule.default = workerThreadsModule;

export default workerThreadsModule;
