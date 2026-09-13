// What `require("node:module")` publishes, and what functions answer.
const m = require("node:module");

const names = Object.getOwnPropertyNames(m)
  .filter((n) => n !== "length" && n !== "name" && n !== "prototype")
  .sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof m[name]);
}

console.log("m is fn:", typeof m === "function");
console.log("m.name:", m.name);
console.log("m.default === m:", m.default === m);
console.log("m.Module === m:", m.Module === m);
console.log("createRequire is fn:", typeof m.createRequire === "function");
console.log("isBuiltin is fn:", typeof m.isBuiltin === "function");
console.log("builtinModules is arr:", Array.isArray(m.builtinModules));
console.log("isBuiltin('fs'):", m.isBuiltin("fs"));
console.log("isBuiltin('node:os'):", m.isBuiltin("node:os"));
console.log("isBuiltin('unknown_xyz'):", m.isBuiltin("unknown_xyz"));

const req = m.createRequire("index.js");
console.log("req is fn:", typeof req === "function");

const inst = new m("foo");
console.log("inst is obj:", typeof inst === "object");
console.log("inst.id:", inst.id);
console.log("inst.exports is obj:", typeof inst.exports === "object");
