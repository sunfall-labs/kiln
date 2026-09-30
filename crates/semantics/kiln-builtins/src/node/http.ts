// kiln-builtins: node:http implementation (Phase 5.2, ADR 0005)

import { Writable } from "node:stream";

const nativeModule =
  typeof process !== "undefined"
    ? (process as any).__kilnNativeModules?.http || null
    : null;

export const METHODS = [
  "ACL",
  "BIND",
  "CHECKOUT",
  "CONNECT",
  "COPY",
  "DELETE",
  "GET",
  "HEAD",
  "LINK",
  "LOCK",
  "M-SEARCH",
  "MERGE",
  "MKACTIVITY",
  "MKCALENDAR",
  "MKCOL",
  "MOVE",
  "NOTIFY",
  "OPTIONS",
  "PATCH",
  "POST",
  "PROPFIND",
  "PROPPATCH",
  "PURGE",
  "PUT",
  "QUERY",
  "REBIND",
  "REPORT",
  "SEARCH",
  "SOURCE",
  "SUBSCRIBE",
  "TRACE",
  "UNBIND",
  "UNLINK",
  "UNLOCK",
  "UNSUBSCRIBE",
];

export const IncomingMessage: any =
  nativeModule && typeof nativeModule.IncomingMessage === "function"
    ? nativeModule.IncomingMessage
    : function IncomingMessage() {};
if (IncomingMessage.prototype && typeof IncomingMessage.prototype.unpipe !== "function") {
  IncomingMessage.prototype.unpipe = function unpipe() {
    return this;
  };
}

export const Server: any =
  nativeModule && typeof nativeModule.Server === "function"
    ? nativeModule.Server
    : function Server(...args: any[]) {
        if (nativeModule && typeof nativeModule.createServer === "function") {
          return nativeModule.createServer(...args);
        }
        throw new Error("node:http requires an HTTP host capability that this realm does not provide");
      };

const NativeServerResponse: any =
  nativeModule && typeof nativeModule.ServerResponse === "function"
    ? nativeModule.ServerResponse
    : null;
const legacyResponseReceivers = new WeakSet<object>();

function legacyResponseMethod(name: string, fallback: Function): void {
  const prototype = NativeServerResponse && NativeServerResponse.prototype;
  if (!prototype) return;
  const descriptor = Object.getOwnPropertyDescriptor(prototype, name);
  const nativeMethod = descriptor && typeof descriptor.value === "function"
    ? descriptor.value
    : prototype[name];
  const delegatedMethod = typeof nativeMethod === "function" ? nativeMethod : null;
  const method = function (this: any, ...args: any[]) {
    return !delegatedMethod || (this && legacyResponseReceivers.has(this))
      ? fallback.apply(this, args)
      : delegatedMethod.apply(this, args);
  };
  Object.defineProperty(method, "name", { value: name, configurable: true });
  Object.defineProperty(method, "length", {
    value: delegatedMethod ? delegatedMethod.length : fallback.length,
    configurable: true,
  });
  Object.defineProperty(prototype, name, descriptor
    ? { ...descriptor, value: method }
    : { value: method, writable: true, configurable: true });
}

export const ServerResponse: any = function ServerResponse(this: any, req?: any, options?: any) {
  if (!NativeServerResponse) {
    throw new Error("node:http requires an HTTP host capability that this realm does not provide");
  }
  if (new.target) return new NativeServerResponse(req, options);
  Object.assign(this, new Writable(options));
  // Native response prototypes do not inherit Writable.prototype. The legacy adapter
  // delegates write/end to Writable, so retain its default hook when the subclass has none.
  if (typeof this._write !== "function") this._write = Writable.prototype._write;
  legacyResponseReceivers.add(this);
  this.req = req;
  this.socket = null;
  this.connection = null;
  this.statusCode = 200;
  this.headersSent = false;
  this._header = null;
  this._legacyHeaders = Object.create(null);
};
Object.defineProperty(ServerResponse, "name", { value: "ServerResponse", configurable: true });
if (NativeServerResponse) {
  Object.setPrototypeOf(ServerResponse, NativeServerResponse);
  ServerResponse.prototype = NativeServerResponse.prototype;
  Object.defineProperty(ServerResponse.prototype, "constructor", {
    value: ServerResponse, writable: true, configurable: true,
  });

  legacyResponseMethod("setHeader", function (this: any, name: any, value: any) {
    this._legacyHeaders[String(name).toLowerCase()] = value;
    return this;
  });
  legacyResponseMethod("getHeader", function (this: any, name: any) {
    return this._legacyHeaders[String(name).toLowerCase()];
  });
  legacyResponseMethod("getHeaders", function (this: any) {
    return Object.assign(Object.create(null), this._legacyHeaders);
  });
  legacyResponseMethod("hasHeader", function (this: any, name: any) {
    return Object.prototype.hasOwnProperty.call(this._legacyHeaders, String(name).toLowerCase());
  });
  legacyResponseMethod("removeHeader", function (this: any, name: any) {
    delete this._legacyHeaders[String(name).toLowerCase()];
  });
  legacyResponseMethod("writeHead", function (this: any, statusCode: any, statusMessage?: any,
                                                  headers?: any) {
    this.statusCode = statusCode;
    if (statusMessage && typeof statusMessage === "object") headers = statusMessage;
    if (headers && typeof headers === "object") {
      for (const name of Object.keys(headers)) this._legacyHeaders[name.toLowerCase()] = headers[name];
    }
    this.headersSent = true;
    return this;
  });
  legacyResponseMethod("write", function (this: any, ...args: any[]) {
    return Writable.prototype.write.apply(this, args);
  });
  legacyResponseMethod("end", function (this: any, ...args: any[]) {
    return Writable.prototype.end.apply(this, args);
  });
  legacyResponseMethod("assignSocket", function (this: any, socket: any) {
    this.socket = socket;
    this.connection = socket;
    return this;
  });
  legacyResponseMethod("destroy", function (this: any, error?: any) {
    return Writable.prototype.destroy.call(this, error);
  });
} else {
  ServerResponse.prototype = Object.create(Writable.prototype);
  Object.defineProperty(ServerResponse.prototype, "constructor", {
    value: ServerResponse, writable: true, configurable: true,
  });
}

export function createServer(...args: any[]): any {
  if (nativeModule && typeof nativeModule.createServer === "function") {
    return nativeModule.createServer(...args);
  }
  throw new Error("node:http requires an HTTP host capability that this realm does not provide");
}

// `http.request(url[, options][, callback])` and `http.get(...)`: node accepts a string, a URL
// or an options object first, an options object second that overrides what the URL said, and a
// callback last. The host underneath takes one options object, so the three spellings are
// reduced to that one here, in the one place both engines pass through.
function requestArguments(args: any[]): any[] {
  let options: any = {};
  let at = 0;
  const first = args[0];
  if (typeof first === "string" || (first && typeof first === "object" && typeof first.href === "string" && typeof first.pathname === "string")) {
    const url = typeof first === "string" ? new URL(first) : first;
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      throw new TypeError(`Protocol "${url.protocol}" not supported. Expected "http:"`);
    }
    options = {
      protocol: url.protocol,
      hostname: url.hostname.startsWith("[") ? url.hostname.slice(1, -1) : url.hostname,
      host: url.hostname.startsWith("[") ? url.hostname.slice(1, -1) : url.hostname,
      port: url.port !== "" ? Number(url.port) : (url.protocol === "https:" ? 443 : 80),
      path: `${url.pathname}${url.search}`,
    };
    if (url.username !== "" || url.password !== "") {
      options.auth = `${decodeURIComponent(url.username)}:${decodeURIComponent(url.password)}`;
    }
    at = 1;
  }
  if (args[at] && typeof args[at] === "object") {
    options = { ...options, ...args[at] };
    at += 1;
  } else if (at === 0) {
    throw new TypeError("HTTP request options must be an object");
  }
  const rest = [options];
  if (typeof args[at] === "function") rest.push(args[at]);
  return rest;
}

export function get(...args: any[]): any {
  if (nativeModule && typeof nativeModule.get === "function") {
    return nativeModule.get(...requestArguments(args));
  }
  throw new Error("node:http requires an HTTP host capability that this realm does not provide");
}

export function request(...args: any[]): any {
  if (nativeModule && typeof nativeModule.request === "function") {
    return nativeModule.request(...requestArguments(args));
  }
  throw new Error("node:http requires an HTTP host capability that this realm does not provide");
}

// `serve` is the WHATWG bootstrap's (web.ts), the same function `Bun.serve` and `Kiln.serve`
// name: one implementation, captured before any program code can replace the globals. Named as
// the bare global rather than through `globalThis`, which would tell the compiled backend's
// liveness pass that this module can reach anything and keep every dispatch group.
declare const Kiln: any;
const bootstrapServe: any = typeof Kiln !== "undefined" ? Kiln.serve : undefined;

export function serve(options: any, callback?: () => void): any {
  if (typeof bootstrapServe !== "function") {
    throw new Error("serve requires an HTTP host capability that this realm does not provide");
  }
  return bootstrapServe(options, callback);
}

const httpNamespace = {
  IncomingMessage,
  METHODS,
  Server,
  ServerResponse,
  createServer,
  default: undefined as any,
  get,
  request,
  serve,
};
httpNamespace.default = httpNamespace;

export default httpNamespace;
