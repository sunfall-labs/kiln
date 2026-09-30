// Shared TypeScript surface over native GC weak cells. Target values and finalization holdings
// live in collector metadata rather than in JavaScript fields, so a WeakRef never keeps its
// target alive and a live registry keeps holdings alive until delivery or unregister.
(function installNativeWeakReferences() {
  const G = globalThis as any;
  const weakOperation: ((operation: number, ...values: any[]) => any) | undefined = G.__kilnSharedWeak;
  delete G.__kilnSharedWeak;
  // The interpreter already has its own WeakRef and FinalizationRegistry implementation.
  if (typeof weakOperation !== "function") return;
  const symbolKeyFor = Symbol.keyFor;

  function canBeHeldWeakly(value: any): boolean {
    if (typeof value === "symbol") return symbolKeyFor.call(Symbol, value) === undefined;
    return (typeof value === "object" && value !== null) || typeof value === "function";
  }

  class WeakRef {
    #sharedToken: number[] | undefined;

    constructor(target: any) {
      if (!canBeHeldWeakly(target)) {
        throw new TypeError("WeakRef: target must be an object or an unregistered symbol");
      }
      const token = weakOperation(0, target);
      if (Array.isArray(token)) this.#sharedToken = token;
      else weakOperation(2, this, target);
    }

    deref(): any {
      return this.#sharedToken === undefined
        ? weakOperation!(3, this)
        : weakOperation!(1, this.#sharedToken);
    }
  }

  class FinalizationRegistry {
    constructor(cleanupCallback: any) {
      if (typeof cleanupCallback !== "function") {
        throw new TypeError("FinalizationRegistry: cleanup callback must be a function");
      }
      weakOperation!(4, this, cleanupCallback);
    }

    register(target: any, heldValue: any): void {
      const unregisterToken = arguments.length > 2 ? arguments[2] : undefined;
      if (!canBeHeldWeakly(target)) {
        throw new TypeError("FinalizationRegistry.prototype.register: target must be an object or an unregistered symbol");
      }
      if (target === heldValue) {
        throw new TypeError("FinalizationRegistry.prototype.register: target and holdings must not be the same");
      }
      if (unregisterToken !== undefined && !canBeHeldWeakly(unregisterToken)) {
        throw new TypeError("FinalizationRegistry.prototype.register: unregisterToken must be an object or an unregistered symbol");
      }
      weakOperation!(5, this, target, heldValue,
        unregisterToken === undefined ? undefined : unregisterToken);
    }

    unregister(unregisterToken: any): boolean {
      if (!canBeHeldWeakly(unregisterToken)) {
        throw new TypeError("FinalizationRegistry.prototype.unregister: unregisterToken must be an object or an unregistered symbol");
      }
      return weakOperation!(6, this, unregisterToken);
    }
  }

  Object.defineProperty(WeakRef.prototype, Symbol.toStringTag,
    { value: "WeakRef", configurable: true });
  Object.defineProperty(FinalizationRegistry.prototype, Symbol.toStringTag,
    { value: "FinalizationRegistry", configurable: true });

  for (const [name, value] of [["WeakRef", WeakRef], ["FinalizationRegistry", FinalizationRegistry]] as const) {
    Object.defineProperty(globalThis, name, {
      value, writable: true, configurable: true, enumerable: false,
    });
  }
})();
