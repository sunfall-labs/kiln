// kiln-builtins: node:timers implementation (Phase 5.2, ADR 0005)

const _setImmediate =
  typeof (globalThis as any).setImmediate === "function"
    ? (globalThis as any).setImmediate
    : (fn: any, ...args: any[]) => globalThis.setTimeout(fn, 0, ...args);

const _clearImmediate =
  typeof (globalThis as any).clearImmediate === "function"
    ? (globalThis as any).clearImmediate
    : (id: any) => globalThis.clearTimeout(id);

export const setTimeout = globalThis.setTimeout;
export const clearTimeout = globalThis.clearTimeout;
export const setInterval = globalThis.setInterval;
export const clearInterval = globalThis.clearInterval;
export const setImmediate = _setImmediate;
export const clearImmediate = _clearImmediate;

const timersNamespace = {
  clearInterval,
  clearImmediate,
  clearTimeout,
  default: undefined as any,
  setImmediate,
  setInterval,
  setTimeout,
};
timersNamespace.default = timersNamespace;

export default timersNamespace;
