// The Function constructor is a computed callable rather than a table-driven namespace, and
// String.prototype methods travel through primitive-special dispatch. Both still expose the
// same reflection metadata as the shared builtin surface.
console.log(Reflect.ownKeys(Function).join(","));
console.log(Function.length, Function.name, typeof Function.prototype);
console.log(String.prototype.indexOf.length, "abc".indexOf.length);
console.log(BigInt.isBuffer === undefined);
console.log(Reflect.ownKeys(Uint8Array).join(","));
console.log(Object.getOwnPropertyNames(Float64Array).join(","));
console.log(Object.getOwnPropertyDescriptor(Uint8Array, "BYTES_PER_ELEMENT").value);
