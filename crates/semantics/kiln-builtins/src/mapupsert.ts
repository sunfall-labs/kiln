// kiln-builtins: `Map.prototype.getOrInsert` and `Map.prototype.getOrInsertComputed` (TC39 Map.upsert proposal Stage 3)
// ADR 0005, ADR 0063 rule 4

(function installMapUpsert() {
  if (typeof Map === "undefined" || !Map.prototype) return;
  const __kilnCollectionSize: ((collection: any) => number) | undefined =
    (globalThis as any).__kilnCollectionSize;
  delete (globalThis as any).__kilnCollectionSize;
  const Proto = Map.prototype;

  const getOrInsert = { getOrInsert(this: any, key: any, value: any): any {
    if (!(this instanceof Map)) {
      throw new TypeError("Method Map.prototype.getOrInsert called on incompatible receiver");
    }
    if (this.has(key)) {
      return this.get(key);
    }
    this.set(key, value);
    return value;
  } }.getOrInsert;

  const getOrInsertComputed = { getOrInsertComputed(this: any, key: any, callback: (key: any) => any): any {
    if (!(this instanceof Map)) {
      throw new TypeError("Method Map.prototype.getOrInsertComputed called on incompatible receiver");
    }
    if (typeof callback !== "function") {
      throw new TypeError("callback must be a function");
    }
    if (this.has(key)) {
      return this.get(key);
    }
    // `CanonicalizeKeyedCollectionKey`: the callback sees the key the map will hold, +0 for -0.
    if (key === 0) key = 0;
    const value = callback.call(undefined, key);
    this.set(key, value);
    return value;
  } }.getOrInsertComputed;

  Object.defineProperty(Proto, "getOrInsert", {
    value: getOrInsert,
    writable: true,
    enumerable: false,
    configurable: true,
  });

  Object.defineProperty(Proto, "getOrInsertComputed", {
    value: getOrInsertComputed,
    writable: true,
    enumerable: false,
    configurable: true,
  });

  if (__kilnCollectionSize !== undefined) {
    const mapGetter = function() { return __kilnCollectionSize(this); };
    Object.defineProperty(mapGetter, "name", { value: "get size", configurable: true });
    Object.defineProperty(Proto, "size", {
      get: mapGetter,
      enumerable: false,
      configurable: true,
    });
  }

  if (typeof Set !== "undefined" && Set.prototype && __kilnCollectionSize !== undefined) {
    const setGetter = function() { return __kilnCollectionSize(this); };
    Object.defineProperty(setGetter, "name", { value: "get size", configurable: true });
    Object.defineProperty(Set.prototype, "size", {
      get: setGetter,
      enumerable: false,
      configurable: true,
    });
  }
})();
