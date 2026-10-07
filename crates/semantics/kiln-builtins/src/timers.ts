// kiln-builtins: the objects `setTimeout`, `setInterval` and `setImmediate` return.
//
// Node hands back a `Timeout` or an `Immediate`, and what makes them more than a number is
// `unref()`: an unreferenced timer does not keep the program alive. Whether it does is one flag
// per timer that the runtime's timer queue owns (`Realm::set_timer_referenced`), the same in the
// interpreter and the native backend.
//
// The handle IS the timer's record, and the engine allocates it: it holds the callback, the
// arguments, the delay and the flags in engine-only slots, the realm's queue entry refers to it
// rather than to a copy, and firing reads the callback from it. So no script runs when a timer
// is armed, which a program that arms a million of them (a thousand sleeping tasks, each waking
// ten times) pays for otherwise, and a timeout that already ran keeps its callback for as long as
// the program holds the handle. That is what lets `refresh()` arm it again, from outside or from
// inside its own callback, exactly as node's does, and a cleared timer drop everything.
//
// All the engine needs from here is the two prototypes below, adopted once by
// `__kilnTimerRef(Timeout.prototype, 5, Immediate.prototype)`; the methods ask the engine
// through `__kilnTimerRef(handle, op)`. A realm that does not install it gets the bare numeric
// identity. `clearTimeout` and friends accept either, reading a handle's slot directly and
// anything else through its `Symbol.toPrimitive`.

(function installTimers() {
  const G = globalThis as any;
  const timerRef: (handle: object, op: number, prototype?: object) => any = G.__kilnTimerRef;
  delete G.__kilnTimerRef;
  if (typeof timerRef !== "function") return;

  // The operations of `__kilnTimerRef`.
  const ID = 0, HAS_REF = 1, REF = 2, UNREF = 3, REFRESH = 4, ADOPT = 5;

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

  timerRef(Timeout.prototype, ADOPT, Immediate.prototype);
})();
