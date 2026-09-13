// kiln-builtins: node:http implementation (Phase 5.2, ADR 0005)

const nativeModule =
  typeof process !== "undefined"
    ? (process as any).__kilnNativeModules?.http ||
      (typeof (process as any).getBuiltinModule === "function"
        ? (process as any).getBuiltinModule("http")
        : null)
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

export function IncomingMessage(this: any, ...args: any[]) {
  if (nativeModule && typeof nativeModule.IncomingMessage === "function") {
    return Reflect.construct(nativeModule.IncomingMessage, args, new.target || IncomingMessage);
  }
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

export function get(...args: any[]): any {
  if (nativeModule && typeof nativeModule.get === "function") {
    return nativeModule.get(...args);
  }
  throw new Error("node:http requires an HTTP host capability that this realm does not provide");
}

export function request(...args: any[]): any {
  if (nativeModule && typeof nativeModule.request === "function") {
    return nativeModule.request(...args);
  }
  throw new Error("node:http requires an HTTP host capability that this realm does not provide");
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
};
httpNamespace.default = httpNamespace;

export default httpNamespace;
