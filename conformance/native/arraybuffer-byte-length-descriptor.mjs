const descriptor = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, "byteLength");
console.log(typeof descriptor.get, descriptor.get.name, descriptor.enumerable, descriptor.configurable);
const buffer = new ArrayBuffer(7);
console.log(descriptor.get.call(buffer));
try {
  descriptor.get.call(ArrayBuffer.prototype);
} catch (error) {
  console.log(error instanceof TypeError);
}
const sharedDescriptor = Object.getOwnPropertyDescriptor(SharedArrayBuffer.prototype, "byteLength");
console.log(typeof sharedDescriptor.get, sharedDescriptor.get.name, sharedDescriptor.enumerable, sharedDescriptor.configurable);
console.log(sharedDescriptor.get.call(new SharedArrayBuffer(9)));
try {
  sharedDescriptor.get.call(buffer);
} catch (error) {
  console.log(error instanceof TypeError);
}
