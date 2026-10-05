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
      const identifierName = (name: string): boolean =>
        /^[$_\p{ID_Start}][$_\u200c\u200d\p{ID_Continue}]*$/u.test(name);
      for (let i = 0; i < frames.length && i < limit; i++) {
        const frame = frames[i];
        const site: any = {};
        const functionName = frame.functionName;
        const methodName = frame.methodName;
        // Bun omits redundant-looking aliases for identifier property calls; keep the property
        // identity available through getMethodName, and show the alias where punctuation makes
        // the invocation key useful diagnostic information (for example Effect's marker key).
        const displayName = methodName && methodName !== functionName && !identifierName(methodName)
          ? `${functionName} [as ${methodName}]`
          : functionName;
        defineHidden(site, "getThis", () => undefined);
        defineHidden(site, "getTypeName", () => null);
        defineHidden(site, "getFunction", () => undefined);
        defineHidden(site, "getFunctionName", () => functionName);
        defineHidden(site, "getMethodName", () => methodName ?? functionName);
        defineHidden(site, "getFileName", () => frame.fileName);
        defineHidden(site, "getLineNumber", () => frame.lineNumber ?? null);
        defineHidden(site, "getColumnNumber", () => frame.columnNumber ?? null);
        defineHidden(site, "getEvalOrigin", () => undefined);
        defineHidden(site, "isToplevel", () => false);
        defineHidden(site, "isEval", () => false);
        defineHidden(site, "isNative", () => false);
        defineHidden(site, "isConstructor", () => false);
        defineHidden(site, "toString", () => {
          const where = frame.lineNumber == null
            ? frame.fileName
            : `${frame.fileName}:${frame.lineNumber}:${frame.columnNumber}`;
          return displayName ? `at ${displayName} (${where})` : `at ${where}`;
        });
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
    // Register these shared TypeScript accessors with each engine's private realm slot. The
    // constructor copies the pair to each Error instance so the observable own-property shape
    // matches Error's [[ErrorData]] behavior without adding `stack` to Error.prototype.
    const stackRegistrationToken = {};
    const errorBrandToken = {};
    captureFrameSnapshots(
      ErrorCtor,
      stackGetter,
      stackSetter,
      stackRegistrationToken,
      errorBrandToken,
    );
    const isError = (value: unknown): boolean =>
      captureFrameSnapshots(value, undefined, errorBrandToken) === true;
    defineHidden(isError, "name", "isError");
    defineHidden(ErrorCtor, "isError", isError);
  }

  // 2. Implement SuppressedError (ES2024 / Explicit Resource Management).
  const NativeSuppressedError = G.SuppressedError;
  function SuppressedError(this: any, error: any, suppressed: any, message?: any): any {
    if (typeof NativeSuppressedError === "function") {
      // The native constructor supplies [[ErrorData]], stack capture, and the Error prototype
      // chain. The TS facade installs the two standard payload properties, which are not part of
      // the native constructor's message-only error allocation path.
      // Allocate the engine-branded Error without forwarding `message`: the native constructor
      // currently provides the Error object/stack but does not install SuppressedError's JS
      // fields. The facade below performs the single required ToString conversion.
      const result = new NativeSuppressedError();
      const targetPrototype =
        typeof new.target === "function" && new.target.prototype != null
          ? new.target.prototype
          : SuppressedError.prototype;
      if (Object.getPrototypeOf(result) !== targetPrototype) {
        if (typeof Object.setPrototypeOf === "function") {
          Object.setPrototypeOf(result, targetPrototype);
        } else {
          result.__proto__ = targetPrototype;
        }
      }
      defineHidden(result, "error", error);
      defineHidden(result, "suppressed", suppressed);
      // Error-style constructors create an own `message` only when a non-undefined message was
      // supplied. The prototype still provides the observable empty string otherwise.
      if (message !== undefined) defineHidden(result, "message", `${message}`);
      return result;
    }
    if (!(this instanceof SuppressedError)) {
      return new (SuppressedError as any)(error, suppressed, message);
    }
    defineHidden(this, "error", error);
    defineHidden(this, "suppressed", suppressed);
    if (message !== undefined) defineHidden(this, "message", `${message}`);
  }

  if (typeof NativeSuppressedError === "function" && NativeSuppressedError.prototype) {
    SuppressedError.prototype = NativeSuppressedError.prototype;
  } else if (typeof Object.setPrototypeOf === "function") {
    Object.setPrototypeOf(SuppressedError.prototype, ErrorCtor.prototype);
  } else {
    SuppressedError.prototype.__proto__ = ErrorCtor.prototype;
  }
  if (typeof Object.setPrototypeOf === "function") {
    Object.setPrototypeOf(SuppressedError, ErrorCtor);
  } else {
    (SuppressedError as any).__proto__ = ErrorCtor;
  }

  defineHidden(SuppressedError.prototype, "name", "SuppressedError");
  defineHidden(SuppressedError.prototype, "message", "");
  defineHidden(SuppressedError.prototype, "constructor", SuppressedError);

  if (typeof NativeSuppressedError === "function") {
    G.SuppressedError = SuppressedError;
  } else {
    defineHidden(G, "SuppressedError", SuppressedError);
  }
})();
