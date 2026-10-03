// kiln-builtins: node:net implementation (Phase 5.2, ADR 0005)

const privateProcess = (globalThis as any).__kilnPrivate?.process ||
  (typeof process !== "undefined" ? (process as any) : null);
const nativeModule =
  privateProcess
    ? privateProcess.__kilnNativeModules?.net || null
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

function socketPause(this: any): any {
  if (!nativeModule) throw new Error("node:net requires the Node execution adapter");
  return nativeModule.__pauseSocket.call(this);
}

function socketResume(this: any): any {
  if (!nativeModule) throw new Error("node:net requires the Node execution adapter");
  return nativeModule.__resumeSocket.call(this);
}

function socketRead(this: any, size?: number): any {
  if (!nativeModule) throw new Error("node:net requires the Node execution adapter");
  if (size === undefined) return nativeModule.__readSocket.call(this);
  const parsedSize = typeof size === "number" && Number.isInteger(size)
    ? size
    : parseInt(size as any, 10);
  if (!Number.isNaN(parsedSize) && parsedSize > 1073741824) {
    const error = new RangeError(
      `The value of "size" is out of range. It must be <= 1GiB. Received ${size}`,
    ) as any;
    error.code = "ERR_OUT_OF_RANGE";
    throw error;
  }
  return nativeModule.__readSocket.call(this, Number.isNaN(parsedSize) ? undefined : parsedSize);
}

const serverPrototypes = new WeakMap<object, object>();
const decoratedServers = new WeakSet<object>();

function serverListening(this: any): boolean {
  if (!nativeModule || typeof nativeModule.__serverIsListening !== "function") {
    throw new Error("node:net server listening state requires a host capability");
  }
  return !!nativeModule.__serverIsListening(this);
}

function decorateServer(server: any, preferredPrototype?: object): any {
  if ((typeof server !== "object" && typeof server !== "function") || server === null) {
    return server;
  }
  if (decoratedServers.has(server) && preferredPrototype === undefined) return server;
  const base = Object.getPrototypeOf(server);
  let prototype = preferredPrototype || serverPrototypes.get(base);
  if (!prototype) {
    prototype = Object.create(base);
    Object.defineProperty(prototype, "listening", {
      configurable: true,
      get: serverListening,
    });
    serverPrototypes.set(base, prototype);
  } else if (!Object.getOwnPropertyDescriptor(prototype, "listening")) {
    Object.defineProperty(prototype, "listening", {
      configurable: true,
      get: serverListening,
    });
  }
  Object.setPrototypeOf(server, prototype);
  decoratedServers.add(server);
  return server;
}

if (privateProcess && typeof privateProcess.__kilnServerDecorator !== "function") {
  Object.defineProperty(privateProcess, "__kilnServerDecorator", {
    configurable: false,
    value: decorateServer,
  });
}

const socketPrototypes = new WeakMap<object, object>();
const decoratedSockets = new WeakSet<object>();

function decorateSocket(socket: any): any {
  if ((typeof socket !== "object" && typeof socket !== "function") || socket === null) {
    return socket;
  }
  if (decoratedSockets.has(socket)) return socket;
  const base = Object.getPrototypeOf(socket);
  let prototype = socketPrototypes.get(base);
  if (!prototype) {
    prototype = Object.create(base);
    Object.defineProperties(prototype, {
      cork: { configurable: true, writable: true, value: socketCork },
      uncork: { configurable: true, writable: true, value: socketUncork },
      pause: { configurable: true, writable: true, value: socketPause },
      resume: { configurable: true, writable: true, value: socketResume },
      read: { configurable: true, writable: true, value: socketRead },
    });
    socketPrototypes.set(base, prototype);
  }
  Object.setPrototypeOf(socket, prototype);
  decoratedSockets.add(socket);
  return socket;
}

if (typeof process !== "undefined" && typeof (process as any).__kilnSocketDecorator !== "function") {
  Object.defineProperty(process, "__kilnSocketDecorator", {
    configurable: false,
    value: decorateSocket,
  });
}

export function createServer(...args: any[]): any {
  if (nativeModule && typeof nativeModule.createServer === "function") {
    // The host decorates accepted sockets before it emits `connection`, regardless of whether
    // listeners are installed here or later with on()/prependListener(). Preserve listener
    // identity and native argument handling by forwarding the original arguments unchanged.
    return decorateServer(nativeModule.createServer(...args));
  }
  throw new Error("node:net requires a net host capability that this realm does not provide");
}

export function connect(...args: any[]): any {
  if (nativeModule && typeof nativeModule.connect === "function") {
    return decorateSocket(nativeModule.connect(...args));
  }
  throw new Error("node:net requires a net host capability that this realm does not provide");
}

export function createConnection(...args: any[]): any {
  return connect(...args);
}

export function isIPv4(s: string): boolean {
  if (typeof s !== "string") return false;
  const parts = s.split(".");
  if (parts.length !== 4) return false;
  for (let i = 0; i < 4; i++) {
    const part = parts[i];
    if (part.length === 0 || part.length > 3) return false;
    if (part.length > 1 && part.charCodeAt(0) === 48) return false;
    let n = 0;
    for (let j = 0; j < part.length; j++) {
      const code = part.charCodeAt(j);
      if (code < 48 || code > 57) return false;
      n = n * 10 + (code - 48);
    }
    if (n > 255) return false;
  }
  return true;
}

export function isIPv6(s: string): boolean {
  if (typeof s !== "string") return false;
  const zoneIndex = s.indexOf("%");
  if (zoneIndex >= 0) {
    if (zoneIndex === s.length - 1) return false;
    s = s.slice(0, zoneIndex);
  }
  if (s.length < 2) return false;

  const doubleColon = s.indexOf("::");
  if (doubleColon >= 0 && s.indexOf("::", doubleColon + 1) >= 0) return false;

  const parts = s.split(":");
  if (parts.length < 3 || parts.length > 9) return false;

  let hasDouble = false;
  let head = 0;
  let tail = parts.length;
  if (parts[0] === "") {
    if (parts.length < 2 || parts[1] !== "") return false;
    head = 2;
    hasDouble = true;
  }
  if (parts[parts.length - 1] === "") {
    if (parts.length < 2 || parts[parts.length - 2] !== "") return false;
    tail = parts.length - 2;
    hasDouble = true;
  }

  let count = 0;
  for (let i = head; i < tail; i++) {
    const part = parts[i];
    if (part === "") {
      if (hasDouble) return false;
      hasDouble = true;
      continue;
    }
    if (i === tail - 1 && part.includes(".")) {
      if (!isIPv4(part)) return false;
      count += 2;
      continue;
    }
    if (part.length < 1 || part.length > 4) return false;
    for (let j = 0; j < part.length; j++) {
      const c = part.charCodeAt(j);
      const isHex = (c >= 48 && c <= 57) || (c >= 65 && c <= 70) || (c >= 97 && c <= 102);
      if (!isHex) return false;
    }
    count++;
  }

  if (doubleColon >= 0) {
    return count < 8;
  }
  return count === 8;
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
