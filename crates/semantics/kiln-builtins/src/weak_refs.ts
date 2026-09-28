// Native GC does not yet clear weak references or deliver cleanup jobs. These
// language objects match the interpreter's current reachability semantics.
(function installNativeWeakReferences() {
  // The native backend publishes this primitive until arraybuffer.ts captures
  // it. The interpreter already has a FinalizationRegistry implementation.
  if (typeof (globalThis as any).__kilnSharedBuffer !== "function") return;

  class WeakRef {
    #target: any;

    constructor(target: any) {
      if ((typeof target !== "object" || target === null) && typeof target !== "function") {
        throw new TypeError("WeakRef: target must be an object or an unregistered symbol");
      }
      this.#target = target;
    }

    deref(): any { return this.#target; }
  }

  class FinalizationRegistry {
    #callback: Function;
    #targets = new WeakMap<object, any[]>();
    #tokens = new WeakMap<object, any[]>();

    constructor(cleanupCallback: any) {
      if (typeof cleanupCallback !== "function") {
        throw new TypeError("FinalizationRegistry: cleanup callback must be a function");
      }
      this.#callback = cleanupCallback;
    }

    register(target: any, heldValue: any, unregisterToken?: any): void {
      if ((typeof target !== "object" || target === null) && typeof target !== "function") {
        throw new TypeError("FinalizationRegistry.prototype.register: target must be an object or an unregistered symbol");
      }
      if (target === heldValue) {
        throw new TypeError("FinalizationRegistry.prototype.register: target and holdings must not be the same");
      }
      if (unregisterToken !== undefined &&
          ((typeof unregisterToken !== "object" || unregisterToken === null) && typeof unregisterToken !== "function")) {
        throw new TypeError("FinalizationRegistry.prototype.register: unregisterToken must be an object or an unregistered symbol");
      }
      const cell = { heldValue, active: true };
      const entries = this.#targets.get(target) || [];
      entries.push(cell);
      this.#targets.set(target, entries);
      if (unregisterToken !== undefined) {
        const tagged = this.#tokens.get(unregisterToken) || [];
        tagged.push(cell);
        this.#tokens.set(unregisterToken, tagged);
      }
    }

    unregister(unregisterToken: any): boolean {
      if ((typeof unregisterToken !== "object" || unregisterToken === null) && typeof unregisterToken !== "function") {
        throw new TypeError("FinalizationRegistry.prototype.unregister: unregisterToken must be an object or an unregistered symbol");
      }
      const cells = this.#tokens.get(unregisterToken);
      if (cells === undefined) return false;
      this.#tokens.delete(unregisterToken);
      for (const cell of cells) cell.active = false;
      return cells.length > 0;
    }
  }

  for (const [name, value] of [["WeakRef", WeakRef], ["FinalizationRegistry", FinalizationRegistry]] as const) {
    Object.defineProperty(globalThis, name, {
      value, writable: true, configurable: true, enumerable: false,
    });
  }
})();
