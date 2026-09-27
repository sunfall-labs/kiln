// kiln-builtins: node's unhandled-rejection tracking (lib/internal/process/promises.js), one
// implementation for both engines (ADR 0060).
//
// Each engine does only what needs its own promise representation: it keeps a promise's
// [[PromiseIsHandled]] bit, notes a promise rejected with nothing attached (HostPromiseRejection-
// Tracker's "reject") and one of those that something handles after it was reported here
// ("handle"), and at the end of every microtask checkpoint -- once the tick queue and the promise
// queue are both empty, which is where node's `processTicksAndRejections` calls
// `processPromiseRejections` -- hands both lists to the function registered below, then drains
// again whatever that queued, and asks again. A rejection handled before its checkpoint ends never
// reaches this file: the engine drops it, as node's `handledRejection` deletes it from
// `maybeUnhandledPromises` before anything looked.
//
// What happens then is node's default mode, `--unhandled-rejections=throw`: an
// `unhandledRejection` listener sees `(reason, promise)` and the program goes on; without one the
// reason -- or, for a reason that is not error-like, an `UnhandledPromiseRejection` naming it --
// is thrown as an uncaught exception, so the engine reports it and exits 1 exactly as for any
// other. A promise reported that way and handled later is a `rejectionHandled` event, or node's
// `PromiseRejectionHandledWarning` when nothing listens.

(function installRejectionTracking() {
  const G = globalThis as any;
  const register: ((process: (rejected: any[], handled: any[]) => void) => void) | undefined =
    G.__kilnPromiseRejections;
  delete G.__kilnPromiseRejections;
  if (typeof register !== "function") return;

  const hasOwn = Object.prototype.hasOwnProperty;
  const functionToString = Function.prototype.toString;
  const symbolToString = Symbol.prototype.toString;

  // Reported and not handled yet; a warned entry becomes `rejectionHandled` when that changes.
  const reported = new WeakMap<object, number>();
  let lastRejectionId = 0;
  let warnedOnce = false;

  // V8's `NoSideEffectsToString`, which node uses to name the reason: nothing it does can run
  // program code, so reporting a rejection cannot itself throw.
  function describe(value: any): string {
    if (typeof value === "symbol") return symbolToString.call(value);
    if (typeof value === "function") return functionToString.call(value);
    if (value === null || typeof value !== "object") return String(value);
    if (Array.isArray(value)) return "[object Array]";
    const proto = Object.getPrototypeOf(value);
    const descriptor = proto === null ? undefined : Object.getOwnPropertyDescriptor(proto, "constructor");
    const constructor = descriptor !== undefined && hasOwn.call(descriptor, "value") ? descriptor.value : undefined;
    const nameDescriptor = typeof constructor === "function" ? Object.getOwnPropertyDescriptor(constructor, "name") : undefined;
    const name = nameDescriptor !== undefined && typeof nameDescriptor.value === "string" ? nameDescriptor.value : "";
    return name === "" ? "[object Object]" : "#<" + name + ">";
  }

  class UnhandledPromiseRejection extends Error {
    code = "ERR_UNHANDLED_REJECTION";
    constructor(reason: any) {
      super(
        "This error originated either by throwing inside of an async function without a catch " +
        "block, or by rejecting a promise which was not handled with .catch(). The promise " +
        'rejected with the reason "' + describe(reason) + '".',
      );
    }
  }
  Object.defineProperty(UnhandledPromiseRejection.prototype, "name", {
    value: "UnhandledPromiseRejection", writable: true, enumerable: false, configurable: true,
  });

  // `triggerUncaughtException(err, true)`: an `uncaughtException` listener takes it, with the
  // origin that says where it came from; otherwise it is thrown out of the checkpoint.
  function uncaught(error: any): void {
    const proc = G.process;
    if (proc && typeof proc.listenerCount === "function" && proc.listenerCount("uncaughtException") > 0) {
      proc.emit("uncaughtException", error, "unhandledRejection");
      return;
    }
    throw error;
  }

  // `process.emitWarning(new PromiseRejectionHandledWarning(id))`: emitted from a tick, and
  // written to stderr by node's own `warning` listener, which is always installed.
  function warnHandledLate(proc: any, id: number): void {
    const warning: any = new Error("Promise rejection was handled asynchronously (rejection id: " + id + ")");
    warning.name = "PromiseRejectionHandledWarning";
    proc.nextTick(() => {
      proc.emit("warning", warning);
      console.error("(node:" + proc.pid + ") " + warning.name + ": " + warning.message);
      if (!warnedOnce) {
        warnedOnce = true;
        console.error("(Use `node --trace-warnings ...` to show where the warning was created)");
      }
    });
  }

  register((rejected: any[], handled: any[]) => {
    const proc = G.process;
    const emits = proc && typeof proc.emit === "function";
    for (let index = 0; index < handled.length; index++) {
      const promise = handled[index];
      const id = reported.get(promise);
      if (id === undefined) continue;
      reported.delete(promise);
      if (!emits || !proc.emit("rejectionHandled", promise)) warnHandledLate(proc, id);
    }
    for (let index = 0; index < rejected.length; index += 2) {
      const promise = rejected[index];
      const reason = rejected[index + 1];
      const id = ++lastRejectionId;
      if (emits && proc.emit("unhandledRejection", reason, promise)) {
        reported.set(promise, id);
        continue;
      }
      const errorLike = typeof reason === "object" && reason !== null && hasOwn.call(reason, "stack");
      uncaught(errorLike ? reason : new UnhandledPromiseRejection(reason));
      reported.set(promise, id);
    }
  });
})();
