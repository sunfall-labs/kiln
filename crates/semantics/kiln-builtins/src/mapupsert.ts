// kiln-builtins: `Map.prototype.getOrInsert` and `Map.prototype.getOrInsertComputed` (TC39 Map.upsert proposal Stage 3)
// ADR 0005, ADR 0063 rule 4

(function installMapUpsert() {
  if (typeof Map === "undefined" || !Map.prototype) return;
  const Proto = Map.prototype;

  function getOrInsert(this: any, key: any, value: any): any {
    if (!(this instanceof Map)) {
      throw new TypeError("Method Map.prototype.getOrInsert called on incompatible receiver");
    }
    if (this.has(key)) {
      return this.get(key);
    }
    this.set(key, value);
    return value;
  }

  function getOrInsertComputed(this: any, key: any, callback: (key: any) => any): any {
    if (!(this instanceof Map)) {
      throw new TypeError("Method Map.prototype.getOrInsertComputed called on incompatible receiver");
    }
    if (typeof callback !== "function") {
      throw new TypeError("callback must be a function");
    }
    if (this.has(key)) {
      return this.get(key);
    }
    const value = callback.call(undefined, key);
    this.set(key, value);
    return value;
  }

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
})();
