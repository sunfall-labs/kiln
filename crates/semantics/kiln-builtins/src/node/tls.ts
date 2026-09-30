// kiln-builtins: node:tls capability facade (ADR 0005).
// Importing TLS does not acquire a transport. A TLS operation requires the host capability;
// libraries that support both ws: and wss: can load their ordinary TCP path in every realm.
const nativeModule = typeof process !== "undefined"
  ? (process as any).__kilnNativeModules?.tls || null : null;

export function connect(...args: any[]): any {
  if (nativeModule && typeof nativeModule.connect === "function")
    return nativeModule.connect(...args);
  throw new Error("node:tls requires a TLS host capability that this realm does not provide");
}

export function createServer(...args: any[]): any {
  if (nativeModule && typeof nativeModule.createServer === "function")
    return nativeModule.createServer(...args);
  throw new Error("node:tls requires a TLS host capability that this realm does not provide");
}

export default { connect, createServer };
