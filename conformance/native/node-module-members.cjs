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

// An executable capability check: a callable-shaped fallback that cannot resolve
// host builtins must diverge here. The absolute filename is valid for Node's
// createRequire contract without depending on this fixture's checkout path.
for (const filename of ["node:path", "relative.cjs", new (require("node:url").URL)("https://example.com/main.cjs")]) {
  try {
    m.createRequire(filename);
    console.log("invalid filename throws:", false);
  } catch (error) {
    console.log("invalid filename throws:", error instanceof TypeError);
  }
}
const req = m.createRequire("/virtual/main.cjs");
console.log("req is fn:", typeof req === "function");
const requiredPath = req("node:path");
console.log("req node:path join is fn:", typeof requiredPath.join === "function");

const inst = new m("foo");
console.log("inst is obj:", typeof inst === "object");
console.log("inst.id:", inst.id);
console.log("inst.exports is obj:", typeof inst.exports === "object");
