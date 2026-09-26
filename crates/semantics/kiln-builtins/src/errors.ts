// kiln-builtins: Error subclass prototypes and SuppressedError (ADR 0005).
//
// Shared by both interpreter and native execution. Guarantees standard ECMAScript
// prototype properties (name, message, constructor) across all standard Error subclasses,
// and implements SuppressedError (ES2024 Explicit Resource Management) with full inheritance.

(function installErrorPrototypes() {
  const G = globalThis as any;

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
        Object.defineProperty(Ctor.prototype, "name", {
          value: name,
          writable: true,
          enumerable: false,
          configurable: true,
        });
        Object.defineProperty(Ctor.prototype, "message", {
          value: "",
          writable: true,
          enumerable: false,
          configurable: true,
        });
        Object.defineProperty(Ctor.prototype, "constructor", {
          value: Ctor,
          writable: true,
          enumerable: false,
          configurable: true,
        });
      }
    }
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

  const ErrorCtor = G.Error || Error;
  if (typeof Object.setPrototypeOf === "function") {
    Object.setPrototypeOf(SuppressedError.prototype, ErrorCtor.prototype);
    Object.setPrototypeOf(SuppressedError, ErrorCtor);
  } else {
    (SuppressedError.prototype as any).__proto__ = ErrorCtor.prototype;
    (SuppressedError as any).__proto__ = ErrorCtor;
  }

  Object.defineProperty(SuppressedError.prototype, "name", {
    value: "SuppressedError",
    writable: true,
    enumerable: false,
    configurable: true,
  });
  Object.defineProperty(SuppressedError.prototype, "message", {
    value: "",
    writable: true,
    enumerable: false,
    configurable: true,
  });
  Object.defineProperty(SuppressedError.prototype, "constructor", {
    value: SuppressedError,
    writable: true,
    enumerable: false,
    configurable: true,
  });

  if (typeof G.SuppressedError === "undefined") {
    Object.defineProperty(G, "SuppressedError", {
      value: SuppressedError,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  } else {
    if (!G.SuppressedError.prototype || G.SuppressedError.prototype === G.Error?.prototype) {
      try {
        const parentProto = G.Error ? G.Error.prototype : Object.prototype;
        G.SuppressedError.prototype = Object.create(parentProto);
      } catch (_) {}
    }
    if (G.SuppressedError.prototype) {
      Object.defineProperty(G.SuppressedError.prototype, "name", {
        value: "SuppressedError",
        writable: true,
        enumerable: false,
        configurable: true,
      });
      Object.defineProperty(G.SuppressedError.prototype, "message", {
        value: "",
        writable: true,
        enumerable: false,
        configurable: true,
      });
      Object.defineProperty(G.SuppressedError.prototype, "constructor", {
        value: G.SuppressedError,
        writable: true,
        enumerable: false,
        configurable: true,
      });
    }
  }
})();
