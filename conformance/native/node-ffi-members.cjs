// What `require("node:ffi")` publishes, and what members answer.
const ffi = require("node:ffi");

const names = Object.getOwnPropertyNames(ffi).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof ffi[name]);
}

console.log("default self-link:", ffi.default === ffi);
console.log("dlopen is function:", typeof ffi.dlopen === "function");
console.log("getRawPointer is function:", typeof ffi.getRawPointer === "function");
console.log("suffix is string:", typeof ffi.suffix === "string");
console.log("toArrayBuffer is function:", typeof ffi.toArrayBuffer === "function");
