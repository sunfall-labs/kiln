// Built-in typed-array constructors are callable objects: their mutable prototype link is
// separate from the `prototype` property used by constructed views.
const parent = { custom: 17 };
Object.setPrototypeOf(Uint8Array, parent);
console.log(Object.getPrototypeOf(Uint8Array) === parent);
console.log(Uint8Array.custom);
console.log(Object.getPrototypeOf(Uint8Array.prototype) === Object.getPrototypeOf(Int8Array.prototype));
console.log(Reflect.ownKeys(Float16Array).includes("from"), Reflect.ownKeys(Float16Array).includes("of"));
console.log(Int8Array.from === Uint8Array.from, Int8Array.of === Uint8Array.of);
console.log(Object.getPrototypeOf(Float16Array).from === Int8Array.from);
const descriptor = Object.getOwnPropertyDescriptor(Uint8Array, "BYTES_PER_ELEMENT");
console.log(descriptor.value, descriptor.writable, descriptor.enumerable, descriptor.configurable);
