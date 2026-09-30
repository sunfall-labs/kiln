// Built-in typed-array constructors are callable objects: their mutable prototype link is
// separate from the `prototype` property used by constructed views.
const parent = { custom: 17 };
Object.setPrototypeOf(Uint8Array, parent);
console.log(Object.getPrototypeOf(Uint8Array) === parent);
console.log(Object.getPrototypeOf(Uint8Array.prototype) === Object.getPrototypeOf(Int8Array.prototype));
console.log(Object.getOwnPropertyDescriptor(Uint8Array, "BYTES_PER_ELEMENT").value);
