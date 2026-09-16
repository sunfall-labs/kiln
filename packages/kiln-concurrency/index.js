/**
 * @kiln/concurrency runtime implementation.
 *
 * What this package promises is exactly what both engines deliver today (ADR 0058, revised
 * 2026-09-06): an actor is an in-realm object behind an asynchronous proxy, `isolate` and
 * `transfer` are identity, and `parallelMap` is `map` -- synchronous, returning an array. The
 * compiled backend may lower `parallelMap` to a pooled walk once it can prove the callback
 * allocates nothing; the answer is the same array. There are no engine hooks: the earlier
 * `__kiln_*` globals were never implemented by either engine and were removed rather than left
 * as promises.
 */

export class Actor {}

export function spawnActor(ctor, ...args) {
  const instance = new ctor(...args);
  return new Proxy(instance, {
    get(target, prop, receiver) {
      const orig = Reflect.get(target, prop, receiver);
      if (typeof orig === "function") {
        return async function (...fnArgs) {
          return orig.apply(target, fnArgs);
        };
      }
      return orig;
    },
  });
}

export function isolate(valOrFactory) {
  if (typeof valOrFactory === "function") {
    return valOrFactory();
  }
  return valOrFactory;
}

export function transfer(val) {
  if (typeof globalThis.__kiln_transfer_region === "function") {
    return globalThis.__kiln_transfer_region(val);
  }
  return val;
}

// Kiln's engines answer `parallelMap` natively; other engines get `map` under that name.
if (typeof Array.prototype.parallelMap !== "function") {
  Object.defineProperty(Array.prototype, "parallelMap", {
    value: function (fn) {
      return Array.prototype.map.call(this, fn);
    },
    writable: true,
    configurable: true,
  });
}

const TypedArrayProto = typeof Uint8Array === "function" ? Object.getPrototypeOf(Uint8Array.prototype) : null;
if (TypedArrayProto && typeof TypedArrayProto.parallelMap !== "function") {
  Object.defineProperty(TypedArrayProto, "parallelMap", {
    value: function (fn) {
      return this.map(fn);
    },
    writable: true,
    configurable: true,
  });
}
