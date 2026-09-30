const descriptor = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, "byteLength");
console.log(typeof descriptor.get, descriptor.get.name, descriptor.enumerable, descriptor.configurable);
const buffer = new ArrayBuffer(7);
console.log(descriptor.get.call(buffer));
try {
  descriptor.get.call(ArrayBuffer.prototype);
} catch (error) {
  console.log(error instanceof TypeError);
}
