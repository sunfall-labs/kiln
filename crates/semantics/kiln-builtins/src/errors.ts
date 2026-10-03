// kiln-builtins: Error subclass prototypes and SuppressedError (ADR 0005).
//
// Shared by both interpreter and native execution. Guarantees standard ECMAScript
// prototype properties (name, message, constructor) across all standard Error subclasses,
// and implements SuppressedError (ES2024 Explicit Resource Management) with full inheritance.

(function installErrorPrototypes() {
  const G = globalThis as any;

  // One descriptor for every property, not a literal per call: `defineProperty` reads its fields
  // and keeps nothing, and a literal per member was the largest share of the garbage every
  // realm's bootstrap made (realms issue 08).
  const hiddenDescriptor: PropertyDescriptor = { value: undefined, writable: true, enumerable: false, configurable: true };
  function defineHidden(target: any, name: string, value: any): void {
    hiddenDescriptor.value = value;
    Object.defineProperty(target, name, hiddenDescriptor);
    hiddenDescriptor.value = undefined;
  }

  // 1. Standard error subclasses: EvalError, RangeError, ReferenceError,
  //    SyntaxError, TypeError, URIError, AggregateError, Error.
  const errorSubclasses: string[] = [
    "EvalError",
    "RangeError",
    "ReferenceError",
    "SyntaxError",
    "TypeError",
    "URIError",
    "AggregateError",
    "Error",
  ];
  for (let i = 0; i < errorSubclasses.length; i++) {
    const name = errorSubclasses[i];
    const Ctor = G[name];
    if (typeof Ctor === "function") {
      if (name !== "Error" && (!Ctor.prototype || Ctor.prototype === G.Error?.prototype)) {
        try {
          const parentProto = G.Error ? G.Error.prototype : Object.prototype;
          Ctor.prototype = Object.create(parentProto);
        } catch (_) {}
      }
      if (Ctor.prototype) {
        defineHidden(Ctor.prototype, "name", name);
        defineHidden(Ctor.prototype, "message", "");
        defineHidden(Ctor.prototype, "constructor", Ctor);
      }
    }
  }

  // The engine-owned primitive returns only activation metadata. Keep public CallSite methods
  // here so both execution engines expose the same ordinary JavaScript facade.
  const ErrorCtor = G.Error || Error;
  const captureFrameSnapshots = ErrorCtor.captureStackTrace;
  if (ErrorCtor.stackTraceLimit === undefined) defineHidden(ErrorCtor, "stackTraceLimit", 10);
  if (typeof captureFrameSnapshots === "function") {
    function stackSites(frames: any[], limit: number): any[] {
      const sites: any[] = [];
      for (let i = 0; i < frames.length && i < limit; i++) {
        const frame = frames[i];
        const site: any = {};
        const functionName = frame.functionName;
        defineHidden(site, "getThis", () => undefined);
        defineHidden(site, "getTypeName", () => null);
        defineHidden(site, "getFunction", () => undefined);
        defineHidden(site, "getFunctionName", () => functionName);
        defineHidden(site, "getMethodName", () => functionName);
        defineHidden(site, "getFileName", () => frame.fileName);
        defineHidden(site, "getLineNumber", () => undefined);
        defineHidden(site, "getColumnNumber", () => undefined);
        defineHidden(site, "getEvalOrigin", () => undefined);
        defineHidden(site, "isToplevel", () => false);
        defineHidden(site, "isEval", () => false);
        defineHidden(site, "isNative", () => false);
        defineHidden(site, "isConstructor", () => false);
        defineHidden(site, "toString", () => functionName
          ? `at ${functionName} (${frame.fileName})`
          : `at ${frame.fileName}`);
        sites.push(site);
      }
      return sites;
    }

    function renderStackTrace(target: any, frames: any[], capturedLimit?: number): any {
      const configuredLimit = ErrorCtor.stackTraceLimit;
      const limit = capturedLimit === undefined
        ? (typeof configuredLimit === "number" ? Math.max(0, Math.floor(configuredLimit)) : 0)
        : capturedLimit;
      const sites = stackSites(frames, limit);
      const prepare = ErrorCtor.prepareStackTrace;
      return typeof prepare === "function"
        ? prepare.call(ErrorCtor, target, sites)
        : `${target.name || "Error"}${target.message ? `: ${target.message}` : ""}${sites.map((site) => `\n    ${site.toString()}`).join("")}`;
    }

    function captureStackTrace(target: any, constructorOpt?: Function): void {
      const frames = captureFrameSnapshots(target, constructorOpt) || [];
      const stack = renderStackTrace(target, frames);
      Object.defineProperty(target, "stack", {
        value: stack,
        writable: true,
        enumerable: false,
        configurable: true,
      });
    }

    for (let i = 0; i < errorSubclasses.length; i++) {
      const Ctor = G[errorSubclasses[i]];
      if (typeof Ctor === "function") defineHidden(Ctor, "captureStackTrace", captureStackTrace);
    }

    const stackGetter = function (this: any): any {
      const frames = captureFrameSnapshots(this, undefined, true);
      if (frames === undefined) return undefined;
      const stack = renderStackTrace(this, frames, frames.length);
      Object.defineProperty(this, "stack", {
        value: stack,
        writable: true,
        enumerable: false,
        configurable: true,
      });
      return stack;
    };
    const stackSetter = function (this: any, value: any): void {
      Object.defineProperty(this, "stack", {
        value,
        writable: true,
        enumerable: false,
        configurable: true,
      });
    };
    Object.defineProperty(ErrorCtor.prototype, "stack", {
      get: stackGetter,
      set: stackSetter,
      configurable: true,
      enumerable: false,
    });
  }

  // 2. Implement SuppressedError (ES2024 / Explicit Resource Management).
  function SuppressedError(this: any, error: any, suppressed: any, message?: any) {
    if (!(this instanceof SuppressedError)) {
      return new (SuppressedError as any)(error, suppressed, message);
    }
    this.error = error;
    this.suppressed = suppressed;
    this.message = message !== undefined ? String(message) : "";
  }

  if (typeof Object.setPrototypeOf === "function") {
    Object.setPrototypeOf(SuppressedError.prototype, ErrorCtor.prototype);
    Object.setPrototypeOf(SuppressedError, ErrorCtor);
  } else {
    (SuppressedError.prototype as any).__proto__ = ErrorCtor.prototype;
    (SuppressedError as any).__proto__ = ErrorCtor;
  }

  defineHidden(SuppressedError.prototype, "name", "SuppressedError");
  defineHidden(SuppressedError.prototype, "message", "");
  defineHidden(SuppressedError.prototype, "constructor", SuppressedError);

  if (typeof G.SuppressedError === "undefined") {
    defineHidden(G, "SuppressedError", SuppressedError);
  } else {
    if (!G.SuppressedError.prototype || G.SuppressedError.prototype === G.Error?.prototype) {
      try {
        const parentProto = G.Error ? G.Error.prototype : Object.prototype;
        G.SuppressedError.prototype = Object.create(parentProto);
      } catch (_) {}
    }
    if (G.SuppressedError.prototype) {
      defineHidden(G.SuppressedError.prototype, "name", "SuppressedError");
      defineHidden(G.SuppressedError.prototype, "message", "");
      defineHidden(G.SuppressedError.prototype, "constructor", G.SuppressedError);
    }
  }
})();
