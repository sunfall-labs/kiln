// What `require("node:buffer")` publishes, and what each name answers.
//
// WHY THIS PROBE EXISTS. `node:buffer` is both the module and the `Buffer` constructor in this
// realm, so the interpreter answers `require("node:buffer").alloc` with a function. The native
// projection built a namespace object out of the three names the shared ABI declared -- `Buffer`,
// `constants`, `default` -- and every other member of the module read as `undefined`. That is the
// silent wrong answer, not a refusal: `typeof buffer.from === "function"` takes the other branch
// and the program carries on with nothing. Seven names were wrong that way, and the same shape of
// bug killed a bundled `postcss` on `path.dirname` for weeks before anybody found it.
//
// The probe does not name the members it checks. It asks the module for its own list, so a member
// added on one side and not the other changes the rows and the two engines stop printing the same
// bytes. Nobody has to remember to extend it.
const buffer = require("node:buffer");

const names = Object.getOwnPropertyNames(buffer).sort();
console.log(names.length);
for (const name of names) {
  console.log(name, typeof buffer[name]);
}

// The identity every `Buffer.isBuffer` shim depends on: the module's constructor is the global.
console.log("same constructor", buffer.Buffer === Buffer);
// And the members really are the operations, not stand-ins that happen to be callable.
console.log("alloc", buffer.alloc(3).length);
console.log("from", buffer.from([104, 105]).toString());
console.log("isBuffer", buffer.isBuffer(buffer.alloc(1)), buffer.isBuffer(1));
console.log("byteLength", buffer.byteLength("hey"));
console.log("concat", buffer.concat([buffer.from([1]), buffer.from([2])]).length);
