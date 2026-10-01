// kiln-builtins: node:net implementation (Phase 5.2, ADR 0005)

const nativeModule =
  typeof process !== "undefined"
    ? (process as any).__kilnNativeModules?.net || null
    : null;

// Public Socket methods are TypeScript facades. The native capability only provides the
// per-socket cork buffer operations; registering here lets HTTP-created sockets share the same
// methods even though their constructors live in the host adapter.
function socketCork(this: any): any {
  if (!nativeModule) throw new Error("node:net requires the Node execution adapter");
  return nativeModule.__corkSocket(this);
}

function socketUncork(this: any): any {
  if (!nativeModule) throw new Error("node:net requires the Node execution adapter");
  return nativeModule.__uncorkSocket(this);
}

const socketPrototypes = new WeakMap<object, object>();

function decorateSocket(socket: any): any {
  const base = Object.getPrototypeOf(socket);
  let prototype = socketPrototypes.get(base);
  if (!prototype) {
    prototype = Object.create(base);
    Object.defineProperties(prototype, {
      cork: { configurable: true, writable: true, value: socketCork },
      uncork: { configurable: true, writable: true, value: socketUncork },
    });
    socketPrototypes.set(base, prototype);
  }
  Object.setPrototypeOf(socket, prototype);
  return socket;
}

if (typeof process !== "undefined") {
  Object.defineProperty(process, "__kilnSocketDecorator", {
    configurable: false,
    value: decorateSocket,
  });
}

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

const v4Seg = "(?:25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9][0-9]|[0-9])";
const v4Str = `(?:${v4Seg}\\.){3}${v4Seg}`;
const IPv4Reg = new RegExp(`^${v4Str}$`);

const v6Seg = "(?:[0-9a-fA-F]{1,4})";
const IPv6Reg = new RegExp(
  "^(?:" +
    `(?:${v6Seg}:){7}(?:${v6Seg}|:)|` +
    `(?:${v6Seg}:){6}(?:${v4Str}|:${v6Seg}|:)|` +
    `(?:${v6Seg}:){5}(?::${v4Str}|(?::${v6Seg}){1,2}|:)|` +
    `(?:${v6Seg}:){4}(?:(?::${v6Seg}){0,1}:${v4Str}|(?::${v6Seg}){1,3}|:)|` +
    `(?:${v6Seg}:){3}(?:(?::${v6Seg}){0,2}:${v4Str}|(?::${v6Seg}){1,4}|:)|` +
    `(?:${v6Seg}:){2}(?:(?::${v6Seg}){0,3}:${v4Str}|(?::${v6Seg}){1,5}|:)|` +
    `(?:${v6Seg}:){1}(?:(?::${v6Seg}){0,4}:${v4Str}|(?::${v6Seg}){1,6}|:)|` +
    `(?::(?:(?::${v6Seg}){0,5}:${v4Str}|(?::${v6Seg}){1,7}|:))` +
    ")(?:%[0-9a-zA-Z]{1,})?$"
);

export function isIPv4(s: string): boolean {
  return typeof s === "string" && IPv4Reg.test(s);
}

export function isIPv6(s: string): boolean {
  return typeof s === "string" && IPv6Reg.test(s);
}

export function isIP(s: string): number {
  if (isIPv4(s)) return 4;
  if (isIPv6(s)) return 6;
  return 0;
}

const netNamespace = {
  connect,
  createConnection,
  createServer,
  default: undefined as any,
  isIP,
  isIPv4,
  isIPv6,
};
netNamespace.default = netNamespace;

export default netNamespace;
