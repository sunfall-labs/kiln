// kiln-builtins: the objects `setTimeout`, `setInterval` and `setImmediate` return.
//
// Node hands back a `Timeout` or an `Immediate`, and what makes them more than a number is
// `unref()`: an unreferenced timer does not keep the program alive. Whether it does is one flag
// per timer that the runtime's timer queue owns (`Realm::set_timer_referenced`), the same in the
// interpreter and the native backend.
//
// The engine allocates the handle itself -- no script runs when a timer is armed, which a
// program that arms a million of them (a thousand sleeping tasks, each waking ten times) pays for
// otherwise. All it needs from here is the two prototypes below, published on the realm's
// engine-private object (`kilnPrivate`, captured in `extensions.ts`; ADR 0068), the one channel a
// program cannot see. The handle holds the timer's identity and whether the program unreferenced
// it in reserved slots that only the engine reads, so the methods here are thin: each asks the
// engine through `__kilnTimerRef(handle, op)`. A realm that publishes no prototypes gets the bare
// numeric identity. `clearTimeout` and friends accept either, reading a handle's slot directly
// and anything else through its `Symbol.toPrimitive`.
//
// `Timeout.prototype.refresh()` holds no copy of the callback, arguments or delay: the runtime's
// timer queue owns the entry, and the engine restarts it in place from the realm's current
// (virtual) time, keeping its identity and referenced state. A timer that already ran or was
// cleared is not re-armed.

(function installTimers() {
  const G = globalThis as any;
  const timerRef: (handle: object, op: number) => any = G.__kilnTimerRef;
  delete G.__kilnTimerRef;
  if (typeof timerRef !== "function") return;

  // The operations of `__kilnTimerRef`.
  const ID = 0, HAS_REF = 1, REF = 2, UNREF = 3, REFRESH = 4;

  class Timeout {
    ref() {
      timerRef(this, REF);
      return this;
    }
    unref() {
      timerRef(this, UNREF);
      return this;
    }
    hasRef() {
      return timerRef(this, HAS_REF);
    }
    refresh() {
      timerRef(this, REFRESH);
      return this;
    }
    close() {
      G.clearTimeout(this);
      return this;
    }
    [Symbol.toPrimitive]() {
      return timerRef(this, ID);
    }
  }

  class Immediate {
    ref() {
      timerRef(this, REF);
      return this;
    }
    unref() {
      timerRef(this, UNREF);
      return this;
    }
    hasRef() {
      return timerRef(this, HAS_REF);
    }
    [Symbol.toPrimitive]() {
      return timerRef(this, ID);
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

  kilnPrivate.timeoutPrototype = Timeout.prototype;
  kilnPrivate.immediatePrototype = Immediate.prototype;
})();
