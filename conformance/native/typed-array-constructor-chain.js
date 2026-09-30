const TypedArray = Object.getPrototypeOf(Uint8Array);
console.log(typeof TypedArray,
  TypedArray === Object.getPrototypeOf(Int8Array),
  TypedArray.prototype === Object.getPrototypeOf(Uint8Array.prototype),
  Object.getPrototypeOf(TypedArray) === Function.prototype,
  Object.getPrototypeOf(TypedArray.prototype) === Object.prototype);
console.log(new Uint8Array(2) instanceof TypedArray,
  new Int16Array(1) instanceof TypedArray,
  new BigInt64Array(1) instanceof TypedArray,
  new DataView(new ArrayBuffer(2)) instanceof TypedArray,
  Buffer.from([1]) instanceof TypedArray,
  "x" instanceof Function.prototype);
try {
  TypedArray(1);
} catch (error) {
  console.log(error.name);
}
try {
  new TypedArray(1);
} catch (error) {
  console.log(error.name);
}
try {
  console.log({} instanceof Function.prototype);
} catch (error) {
  console.log(error.name);
}
