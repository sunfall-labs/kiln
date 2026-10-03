// kiln-builtins: the objects `setTimeout`, `setInterval` and `setImmediate` return.
//
// Node hands back a `Timeout` or an `Immediate`, and what makes them more than a number is
// `unref()`: an unreferenced timer does not keep the program alive. Whether it does is one flag
// per timer that the runtime's timer queue owns (`Realm::set_timer_referenced`), the same in the
// interpreter and the native backend, and `__kilnTimerRef(id, referenced)` is the only way to
// move it. Everything else here -- the object, `hasRef`, `toPrimitive` -- is TypeScript.
//
// The engines ask for a handle by calling `__kilnTimerHandle(id, kind)`, which stays on the
// global object (non-enumerable) because a timer is armed from native code that has no other way
// to find it: `kind` is 0 for a timeout, 1 for an interval, 2 for an immediate. A realm that does
// not install it gets the bare numeric identity. `clearTimeout` and friends accept either, by
// reading a handle's `Symbol.toPrimitive`.
//
// `refresh()` is not here: it re-arms the same callback and arguments, which a handle created
// after the timer was armed does not hold.

(function installTimers() {
  const G = globalThis as any;
  const timerRef: (id: number, referenced: boolean) => void = G.__kilnTimerRef;
  delete G.__kilnTimerRef;
  if (typeof timerRef !== "function") return;

  class Timeout {
    #id: number;
    #ref = true;
    constructor(id: number) {
      this.#id = id;
    }
    ref() {
      if (!this.#ref) {
        this.#ref = true;
        timerRef(this.#id, true);
      }
      return this;
    }
    unref() {
      if (this.#ref) {
        this.#ref = false;
        timerRef(this.#id, false);
      }
      return this;
    }
    hasRef() {
      return this.#ref;
    }
    close() {
      G.clearTimeout(this.#id);
      return this;
    }
    [Symbol.toPrimitive]() {
      return this.#id;
    }
  }

  class Immediate {
    #id: number;
    #ref = true;
    constructor(id: number) {
      this.#id = id;
    }
    ref() {
      if (!this.#ref) {
        this.#ref = true;
        timerRef(this.#id, true);
      }
      return this;
    }
    unref() {
      if (this.#ref) {
        this.#ref = false;
        timerRef(this.#id, false);
      }
      return this;
    }
    hasRef() {
      return this.#ref;
    }
    [Symbol.toPrimitive]() {
      return this.#id;
    }
  }

  // `using timer = setTimeout(...)` clears it; only where the realm has `Symbol.dispose`.
  if (typeof Symbol.dispose === "symbol") {
    Object.defineProperty(Timeout.prototype, Symbol.dispose, {
      value: function (this: Timeout) { this.close(); },
      writable: true, enumerable: false, configurable: true,
    });
    Object.defineProperty(Immediate.prototype, Symbol.dispose, {
      value: function (this: any) { G.clearImmediate(this); },
      writable: true, enumerable: false, configurable: true,
    });
  }

  Object.defineProperty(G, "__kilnTimerHandle", {
    value: (id: number, kind: number) => (kind === 2 ? new Immediate(id) : new Timeout(id)),
    writable: true,
    enumerable: false,
    configurable: true,
  });
})();
