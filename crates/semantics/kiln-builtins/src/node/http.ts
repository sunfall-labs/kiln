// kiln-builtins: node:http implementation (Phase 5.2, ADR 0005)
import "node:net";

import { Readable, Writable } from "node:stream";

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
const incomingMessagePrototype = nativeModule?.__incomingMessagePrototype || IncomingMessage.prototype;
if (incomingMessagePrototype && typeof Readable === "function") {
  if (Object.getPrototypeOf(incomingMessagePrototype) !== Readable.prototype) {
    Object.setPrototypeOf(incomingMessagePrototype, Readable.prototype);
  }
  if (IncomingMessage.prototype !== incomingMessagePrototype) {
    IncomingMessage.prototype = incomingMessagePrototype;
  }
  // Native EventEmitter methods own listener storage for HTTP host objects. Keep that storage
  // authoritative, while applying Readable's data-listener transition through the shared state.
  for (const name of ["on", "addListener", "once"]) {
    const nativeRegister = incomingMessagePrototype[name];
    if (typeof nativeRegister !== "function") continue;
    Object.defineProperty(incomingMessagePrototype, name, {
      configurable: true,
      writable: true,
      value: function (this: any, event: any, listener: any): any {
        const result = nativeRegister.call(this, event, listener);
        if (event === "data" && this._readableState.flowing !== false) this.resume();
        return result;
      },
    });
  }
  const nativeEmit = incomingMessagePrototype.emit;
  const finishNativeIncoming = nativeModule?.__incomingMessageEnded;
  if (typeof nativeEmit === "function") {
    Object.defineProperty(incomingMessagePrototype, "emit", {
      configurable: true,
      writable: true,
      value: function (this: any, event: any, ...args: any[]): any {
        let result: any;
        let listenerError: any;
        let listenerFailed = false;
        try {
          result = nativeEmit.call(this, event, ...args);
        } catch (error) {
          listenerFailed = true;
          listenerError = error;
        }
        if (event === "end" && this._readableState.endEmitted &&
            typeof finishNativeIncoming === "function") {
          try {
            finishNativeIncoming.call(nativeModule, this, listenerFailed);
          } catch (completionError) {
            if (!listenerFailed) throw completionError;
          }
        }
        if (listenerFailed) throw listenerError;
        return result;
      },
    });
  }
  const nativeSetEncoding = incomingMessagePrototype.setEncoding;
  if (typeof nativeSetEncoding === "function") {
    Object.defineProperty(incomingMessagePrototype, "setEncoding", {
      configurable: true,
      writable: true,
      value: function (this: any, encoding: any): any {
        const result = nativeSetEncoding.call(this, encoding);
        const state = this._readableState;
        const normalized = String(encoding).toLowerCase();
        state.encoding = normalized === "utf-8" ? "utf-8" : "utf8";
        return result;
      },
    });
  }
  // Native IncomingMessage objects are allocated by the HTTP host rather than by the
  // TypeScript Readable constructor. Run that constructor's existing initializer lazily so
  // native-created instances get the same state and public flags as ordinary Readables.
  Object.defineProperty(incomingMessagePrototype, "_readableState", {
    configurable: true,
    get: function (this: object): any {
      const initialized: any = {};
      Readable.call(initialized);
      const state = initialized._readableState;
      Object.defineProperty(this, "_readableState", {
        configurable: true,
        enumerable: true,
        writable: true,
        value: state,
      });
      const incoming: any = this;
      if (incoming.readable === undefined) incoming.readable = initialized.readable;
      if (incoming.readableEnded === undefined) incoming.readableEnded = initialized.readableEnded;
      if (incoming.destroyed === undefined) incoming.destroyed = initialized.destroyed;
      return state;
    },
  });
}
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
    if (!delegatedMethod || (this && legacyResponseReceivers.has(this))) {
      return fallback.apply(this, args);
    }
    return delegatedMethod.apply(this, args);
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
    // The native backend specializes methods installed on its ServerResponse prototype, so the
    // prototype compatibility wrapper cannot intercept `res.end(callback)`. Adapt the per-request
    // instance before user code receives it; the host emits `finish` at the real response
    // completion boundary.
    let listenerIndex = -1;
    for (let index = args.length - 1; index >= 0; index--) {
      if (typeof args[index] === "function") {
        listenerIndex = index;
        break;
      }
    }
    if (listenerIndex >= 0) {
      const listener = args[listenerIndex];
      args[listenerIndex] = function (this: any, ...requestArgs: any[]) {
        const response = requestArgs[1];
        if (response && typeof response.end === "function") {
          const nativeEnd = response.end;
          response.end = function (this: any, ...endArgs: any[]) {
            const callback = endArgs.length > 0 && typeof endArgs[endArgs.length - 1] === "function"
              ? endArgs.pop()
              : undefined;
            if (callback !== undefined) this.once("finish", callback);
            try {
              return nativeEnd.apply(this, endArgs);
            } catch (error) {
              if (callback !== undefined) this.removeListener("finish", callback);
              throw error;
            }
          };
        }
        return listener.apply(this, requestArgs);
      };
    }
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
