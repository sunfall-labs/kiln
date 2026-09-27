// kiln-builtins: `DisposableStack` and `AsyncDisposableStack` (ES2024 / Stage 3 Explicit Resource Management)
// ADR 0005, ADR 0063 rule 4

(function installDisposable() {
  if (typeof (Symbol as any).dispose !== "symbol") {
    Object.defineProperty(Symbol, "dispose", {
      value: Symbol("Symbol.dispose"),
      writable: false,
      enumerable: false,
      configurable: false,
    });
  }
  if (typeof (Symbol as any).asyncDispose !== "symbol") {
    Object.defineProperty(Symbol, "asyncDispose", {
      value: Symbol("Symbol.asyncDispose"),
      writable: false,
      enumerable: false,
      configurable: false,
    });
  }

  function combineErrors(error: any, newError: any): any {
    const SuppressedErrorCtor = (globalThis as any).SuppressedError;
    if (typeof SuppressedErrorCtor === "function") {
      return new SuppressedErrorCtor(newError, error);
    }
    const err = new Error("An error was suppressed during disposal") as any;
    err.name = "SuppressedError";
    err.error = newError;
    err.suppressed = error;
    return err;
  }

  if (typeof (globalThis as any).SuppressedError === "undefined") {
    class SuppressedError extends Error {
      error: any;
      suppressed: any;
      constructor(error: any, suppressed: any, message?: string) {
        super(message !== undefined ? String(message) : "An error was suppressed during disposal");
        this.name = "SuppressedError";
        this.error = error;
        this.suppressed = suppressed;
        Object.setPrototypeOf(this, SuppressedError.prototype);
      }
    }
    Object.defineProperty(globalThis, "SuppressedError", {
      value: SuppressedError,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  }

  interface DisposableResource {
    call: () => any;
  }

  const disposableStackState = new WeakMap<DisposableStack, {
    status: "pending" | "disposed";
    stack: DisposableResource[];
  }>();

  class DisposableStack {
    constructor() {
      disposableStackState.set(this, {
        status: "pending",
        stack: [],
      });
    }

    get disposed(): boolean {
      const state = disposableStackState.get(this);
      if (!state) {
        throw new TypeError("Method DisposableStack.prototype.disposed called on incompatible receiver");
      }
      return state.status === "disposed";
    }

    use<T>(value: T): T {
      const state = disposableStackState.get(this);
      if (!state) {
        throw new TypeError("Method DisposableStack.prototype.use called on incompatible receiver");
      }
      if (state.status === "disposed") {
        throw new ReferenceError("Cannot call DisposableStack.prototype.use on an already-disposed DisposableStack");
      }
      if (value === null || value === undefined) {
        return value;
      }
      const method = (value as any)[(Symbol as any).dispose];
      if (typeof method !== "function") {
        throw new TypeError("Property [Symbol.dispose] is not a function");
      }
      state.stack.push({
        call: () => method.call(value),
      });
      return value;
    }

    adopt<T>(value: T, onDispose: (val: T) => void): T {
      const state = disposableStackState.get(this);
      if (!state) {
        throw new TypeError("Method DisposableStack.prototype.adopt called on incompatible receiver");
      }
      if (state.status === "disposed") {
        throw new ReferenceError("Cannot call DisposableStack.prototype.adopt on an already-disposed DisposableStack");
      }
      if (typeof onDispose !== "function") {
        throw new TypeError("onDispose must be a function");
      }
      state.stack.push({
        call: () => onDispose.call(undefined, value),
      });
      return value;
    }

    defer(onDispose: () => void): void {
      const state = disposableStackState.get(this);
      if (!state) {
        throw new TypeError("Method DisposableStack.prototype.defer called on incompatible receiver");
      }
      if (state.status === "disposed") {
        throw new ReferenceError("Cannot call DisposableStack.prototype.defer on an already-disposed DisposableStack");
      }
      if (typeof onDispose !== "function") {
        throw new TypeError("onDispose must be a function");
      }
      state.stack.push({
        call: () => onDispose.call(undefined),
      });
    }

    move(): DisposableStack {
      const state = disposableStackState.get(this);
      if (!state) {
        throw new TypeError("Method DisposableStack.prototype.move called on incompatible receiver");
      }
      if (state.status === "disposed") {
        throw new ReferenceError("Cannot call DisposableStack.prototype.move on an already-disposed DisposableStack");
      }
      const newStack = new DisposableStack();
      const newState = disposableStackState.get(newStack)!;
      newState.stack = state.stack;
      state.stack = [];
      state.status = "disposed";
      return newStack;
    }

    dispose(): void {
      const state = disposableStackState.get(this);
      if (!state) {
        throw new TypeError("Method DisposableStack.prototype.dispose called on incompatible receiver");
      }
      if (state.status === "disposed") {
        return;
      }
      state.status = "disposed";
      let hasError = false;
      let error: any = undefined;
      while (state.stack.length > 0) {
        const item = state.stack.pop()!;
        try {
          item.call();
        } catch (e) {
          if (hasError) {
            error = combineErrors(error, e);
          } else {
            error = e;
            hasError = true;
          }
        }
      }
      if (hasError) {
        throw error;
      }
    }

    [(Symbol as any).dispose](): void {
      this.dispose();
    }
  }

  Object.defineProperty(DisposableStack.prototype, Symbol.toStringTag, {
    value: "DisposableStack",
    configurable: true,
  });

  interface AsyncDisposableResource {
    call: () => any;
  }

  const asyncDisposableStackState = new WeakMap<AsyncDisposableStack, {
    status: "pending" | "disposed";
    stack: AsyncDisposableResource[];
  }>();

  class AsyncDisposableStack {
    constructor() {
      asyncDisposableStackState.set(this, {
        status: "pending",
        stack: [],
      });
    }

    get disposed(): boolean {
      const state = asyncDisposableStackState.get(this);
      if (!state) {
        throw new TypeError("Method AsyncDisposableStack.prototype.disposed called on incompatible receiver");
      }
      return state.status === "disposed";
    }

    use<T>(value: T): T {
      const state = asyncDisposableStackState.get(this);
      if (!state) {
        throw new TypeError("Method AsyncDisposableStack.prototype.use called on incompatible receiver");
      }
      if (state.status === "disposed") {
        throw new ReferenceError("Cannot call AsyncDisposableStack.prototype.use on an already-disposed AsyncDisposableStack");
      }
      if (value === null || value === undefined) {
        return value;
      }
      let method: any = (value as any)[(Symbol as any).asyncDispose];
      if (typeof method !== "function") {
        method = (value as any)[(Symbol as any).dispose];
        if (typeof method !== "function") {
          throw new TypeError("Property [Symbol.asyncDispose] or [Symbol.dispose] is not a function");
        }
      }
      state.stack.push({
        call: () => method.call(value),
      });
      return value;
    }

    adopt<T>(value: T, onDispose: (val: T) => any): T {
      const state = asyncDisposableStackState.get(this);
      if (!state) {
        throw new TypeError("Method AsyncDisposableStack.prototype.adopt called on incompatible receiver");
      }
      if (state.status === "disposed") {
        throw new ReferenceError("Cannot call AsyncDisposableStack.prototype.adopt on an already-disposed AsyncDisposableStack");
      }
      if (typeof onDispose !== "function") {
        throw new TypeError("onDispose must be a function");
      }
      state.stack.push({
        call: () => onDispose.call(undefined, value),
      });
      return value;
    }

    defer(onDispose: () => any): void {
      const state = asyncDisposableStackState.get(this);
      if (!state) {
        throw new TypeError("Method AsyncDisposableStack.prototype.defer called on incompatible receiver");
      }
      if (state.status === "disposed") {
        throw new ReferenceError("Cannot call AsyncDisposableStack.prototype.defer on an already-disposed AsyncDisposableStack");
      }
      if (typeof onDispose !== "function") {
        throw new TypeError("onDispose must be a function");
      }
      state.stack.push({
        call: () => onDispose.call(undefined),
      });
    }

    move(): AsyncDisposableStack {
      const state = asyncDisposableStackState.get(this);
      if (!state) {
        throw new TypeError("Method AsyncDisposableStack.prototype.move called on incompatible receiver");
      }
      if (state.status === "disposed") {
        throw new ReferenceError("Cannot call AsyncDisposableStack.prototype.move on an already-disposed AsyncDisposableStack");
      }
      const newStack = new AsyncDisposableStack();
      const newState = asyncDisposableStackState.get(newStack)!;
      newState.stack = state.stack;
      state.stack = [];
      state.status = "disposed";
      return newStack;
    }

    async disposeAsync(): Promise<void> {
      const state = asyncDisposableStackState.get(this);
      if (!state) {
        throw new TypeError("Method AsyncDisposableStack.prototype.disposeAsync called on incompatible receiver");
      }
      if (state.status === "disposed") {
        return;
      }
      state.status = "disposed";
      let hasError = false;
      let error: any = undefined;
      while (state.stack.length > 0) {
        const item = state.stack.pop()!;
        try {
          await item.call();
        } catch (e) {
          if (hasError) {
            error = combineErrors(error, e);
          } else {
            error = e;
            hasError = true;
          }
        }
      }
      if (hasError) {
        throw error;
      }
    }

    [(Symbol as any).asyncDispose](): Promise<void> {
      return this.disposeAsync();
    }
  }

  Object.defineProperty(AsyncDisposableStack.prototype, Symbol.toStringTag, {
    value: "AsyncDisposableStack",
    configurable: true,
  });

  if (typeof (globalThis as any).DisposableStack === "undefined") {
    Object.defineProperty(globalThis, "DisposableStack", {
      value: DisposableStack,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  }

  if (typeof (globalThis as any).AsyncDisposableStack === "undefined") {
    Object.defineProperty(globalThis, "AsyncDisposableStack", {
      value: AsyncDisposableStack,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  }
})();
