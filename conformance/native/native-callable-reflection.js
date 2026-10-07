const samples = [
  [Object.keys, 1],
  [Object.getOwnPropertyDescriptor, 2],
  [Function.prototype.call, 1],
  [Function.prototype.apply, 2],
  [Function.prototype.bind, 1],
  [/x/.test, 1],
  [/x/.exec, 1],
  [/x/.toString, 0],
];

for (const [fn, length] of samples) {
  const descriptor = Object.getOwnPropertyDescriptor(fn, "length");
  console.log(fn.length, descriptor.value, descriptor.writable,
    descriptor.enumerable, descriptor.configurable);
}
console.log(Object.keys.hasOwnProperty("length"));
console.log(Object.prototype.hasOwnProperty.call(Function.prototype, "call"),
  Object.prototype.hasOwnProperty.call(Function.prototype, "constructor"));
console.log(/x/.hasOwnProperty("lastIndex"), /x/.hasOwnProperty("source"));
