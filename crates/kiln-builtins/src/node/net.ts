// kiln-builtins: node:net implementation (Phase 5.2, ADR 0005)

const nativeModule =
  typeof process !== "undefined"
    ? (process as any).__kilnNativeModules?.net ||
      (typeof (process as any).getBuiltinModule === "function"
        ? (process as any).getBuiltinModule("net")
        : null)
    : null;

export function createServer(...args: any[]): any {
  if (nativeModule && typeof nativeModule.createServer === "function") {
    return nativeModule.createServer(...args);
  }
  throw new Error("node:net requires a net host capability that this realm does not provide");
}

export function connect(...args: any[]): any {
  if (nativeModule && typeof nativeModule.connect === "function") {
    return nativeModule.connect(...args);
  }
  throw new Error("node:net requires a net host capability that this realm does not provide");
}

export function createConnection(...args: any[]): any {
  if (nativeModule && typeof nativeModule.createConnection === "function") {
    return nativeModule.createConnection(...args);
  }
  return connect(...args);
}

const netNamespace = {
  connect,
  createConnection,
  createServer,
  default: undefined as any,
};
netNamespace.default = netNamespace;

export default netNamespace;
