/**
 * @kiln/concurrency runtime implementation.
 */

export class Actor {}

export function spawnActor(ctor, ...args) {
  const instance = new ctor(...args);
  
  // Return an asynchronous proxy wrapping the instance methods
  return new Proxy(instance, {
    get(target, prop, receiver) {
      const orig = Reflect.get(target, prop, receiver);
      if (typeof orig === "function") {
        return async function (...fnArgs) {
          // If native Kiln actor binding is active, dispatch to worker realm;
          // otherwise execute in async turn.
          if (typeof globalThis.__kiln_actor_dispatch === "function") {
            return globalThis.__kiln_actor_dispatch(target, prop, fnArgs);
          }
          return orig.apply(target, fnArgs);
        };
      }
      return orig;
    }
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
    globalThis.__kiln_transfer_region(val);
  }
  return val;
}

// Augment Array.prototype.parallelMap
if (typeof Array.prototype.parallelMap !== "function") {
  Object.defineProperty(Array.prototype, "parallelMap", {
    value: async function (fn) {
      if (typeof globalThis.__kiln_parallel_map === "function") {
        return globalThis.__kiln_parallel_map(this, fn);
      }
      // Dual-mode fallback: evaluate concurrently via Promise.all
      return Promise.all(this.map((item, idx) => fn(item, idx)));
    },
    writable: true,
    configurable: true,
  });
}
