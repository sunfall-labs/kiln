// The Function constructor is a computed callable rather than a table-driven namespace, and
// String.prototype methods travel through primitive-special dispatch. Both still expose the
// same reflection metadata as the shared builtin surface.
console.log(Reflect.ownKeys(Function).join(","));
console.log(Function.length, Function.name, typeof Function.prototype);
console.log(String.prototype.indexOf.length, "abc".indexOf.length);
console.log(BigInt.isBuffer === undefined);
