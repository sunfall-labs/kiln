// Intrinsic prototype methods are real own properties, even though Kiln stores them in compact
// native method tables rather than mutable objects.
const push = Object.getOwnPropertyDescriptor(Array.prototype, "push");
console.log(
  typeof push.value,
  push.writable,
  push.enumerable,
  push.configurable,
  Object.getOwnPropertyNames(Array.prototype).includes("push"),
  Reflect.ownKeys(Array.prototype).includes("push"),
);

// Copy-on-write libraries such as Immer walk descriptors to avoid invoking overridden own
// properties. Their proxy must still recover and invoke an inherited array method.
const source = [1, 2, 3];
const forwarded = new Proxy(source, {
  get(target, key, receiver) {
    if (Object.prototype.hasOwnProperty.call(target, key)) return target[key];
    let prototype = Object.getPrototypeOf(target);
    while (prototype) {
      const descriptor = Object.getOwnPropertyDescriptor(prototype, key);
      if (descriptor) {
        return "value" in descriptor ? descriptor.value : descriptor.get?.call(receiver);
      }
      prototype = Object.getPrototypeOf(prototype);
    }
    return undefined;
  },
});
forwarded.push(4);
console.log(source.join(","));
