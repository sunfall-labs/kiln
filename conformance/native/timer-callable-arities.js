for (const name of ["queueMicrotask", "setTimeout", "setInterval", "clearTimeout",
                   "clearInterval", "setImmediate", "clearImmediate"]) {
  const fn = globalThis[name];
  const descriptor = Object.getOwnPropertyDescriptor(fn, "length");
  console.log(name, fn.length, descriptor.value, descriptor.writable,
              descriptor.enumerable, descriptor.configurable);
}
