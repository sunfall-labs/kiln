const TypedArray = Object.getPrototypeOf(Uint8Array);
console.log(new Uint8Array(2) instanceof TypedArray,
  new Int16Array(1) instanceof TypedArray,
  new DataView(new ArrayBuffer(2)) instanceof TypedArray,
  Buffer.from([1]) instanceof TypedArray);
