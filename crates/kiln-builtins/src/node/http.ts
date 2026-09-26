// kiln-builtins: node:http implementation (Phase 5.2, ADR 0005)

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

export const ServerResponse: any =
  nativeModule && typeof nativeModule.ServerResponse === "function"
    ? nativeModule.ServerResponse
    : function ServerResponse() {
        throw new Error("node:http requires an HTTP host capability that this realm does not provide");
      };

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
// name: one implementation, captured before any program code can replace the globals.
const bootstrapServe: any = (globalThis as any).Kiln?.serve;

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

