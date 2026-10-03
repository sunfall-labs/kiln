import process from "node:process";

for (const fn of [process.hrtime, process.hrtime.bigint]) {
  const descriptor = Object.getOwnPropertyDescriptor(fn, "prototype");
  console.log(fn.name, fn.length, fn.prototype.constructor === fn,
    descriptor.writable, descriptor.enumerable, descriptor.configurable);
}
const original = process.hrtime.bigint;
const descriptor = Object.getOwnPropertyDescriptor(process.hrtime, "bigint");
console.log(descriptor.writable, descriptor.enumerable, descriptor.configurable);
Object.defineProperty(process.hrtime, "bigint", { ...descriptor, value: () => 123n });
console.log(process.hrtime.bigint() === 123n);
Object.defineProperty(process.hrtime, "bigint", descriptor);
console.log(process.hrtime.bigint === original, typeof process.hrtime.bigint());
console.log(Array.isArray(new process.hrtime()), typeof new process.hrtime.bigint());
