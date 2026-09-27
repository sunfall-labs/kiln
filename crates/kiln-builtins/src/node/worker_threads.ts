// kiln-builtins: node:worker_threads implementation (Phase 5.2, ADR 0005)

export class Worker {
  constructor(_filename: string | any, _options?: any) {
    throw new Error("node:worker_threads requires an isolated realm worker capability");
  }
}

export const MessageChannel = typeof (globalThis as any).MessageChannel === "function"
  ? (globalThis as any).MessageChannel
  : class MessageChannel {
      port1: any = {};
      port2: any = {};
    };

export const MessagePort = typeof (globalThis as any).MessagePort === "function"
  ? (globalThis as any).MessagePort
  : class MessagePort {};

export const BroadcastChannel = typeof (globalThis as any).BroadcastChannel === "function"
  ? (globalThis as any).BroadcastChannel
  : class BroadcastChannel {
      readonly name: string;
      constructor(name: string) {
        this.name = String(name);
      }
      postMessage(_message: any): void {}
      close(): void {}
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
export function markAsUncloneable(_obj: any): void {}
export function moveMessagePortToContext(_port: any, _context: any): any {
  return _port;
}
export function receiveMessageOnPort(_port: any): any {
  return undefined;
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
