// Why `postcss` diverges natively, reduced to eleven lines. Run with `--node`.
//
// The native `node:path` namespace publishes the *interpreter's* full export-name list and fills
// only the five names the native side actually implements -- `isAbsolute`, `join`, `resolve`,
// `sep`, `default`. Every other name is present, enumerable, and reads `undefined`. Same for
// `node:fs`, which fills `existsSync` and `promises` and publishes `readFileSync` and
// `realpathSync` alongside them.
//
// That is a wrong answer rather than a refusal, and the two halves of the loader disagree about
// it: a static `import { dirname } from "node:path"` refuses by name with "native Node execution
// does not yet materialize the `node:path` export `dirname`", while `require("path").dirname` --
// which is what an esbuild bundle emits -- hands back undefined and lets the program run on.
//
// node and the interpreter:
//   path.dirname function ... destructured function function string ... dirname call a/b
// native:
//   path.dirname undefined ... destructured undefined undefined string
//   dirname call THREW TypeError: a undefined is not a function
//
// In `postcss` the undefined arrives four frames from where it is called. `input.js` computes
// `pathAvailable` from `resolve && isAbsolute`, both of which the native side does have, so it is
// true and the constructor goes on to build a `PreviousMap`. `previous-map.js` destructures
// `dirname` from the same module without a guard of its own and calls it on the last line of its
// constructor. The whole bundle dies on the first `postcss.parse(css)` with "a undefined is not a
// function", which is why the receiver in the diagnostic is a function -- `postcss` itself -- and
// why `postcss.parse` looked like the missing member. It is not: `parse` is there and is called.
//
// Behind this one, `String.fromCharCode` refuses, which is already in the backlog table. Supply
// `dirname`, `relative` and `fromCharCode` and the native backend and the interpreter agree
// byte-for-byte through the whole postcss pipeline -- measured, by polyfilling all three into a
// copy of the bundle.
//
// The fix needs a method id for `path.dirname` and `path.relative`, which the branch that found
// this was not allowed to allocate. Publishing only the names the native side fills would at
// least stop the namespace advertising members it does not have, but it would not make `postcss`
// run: postcss reads those names, it does not enumerate them.
const path = require("path");
const fs = require("fs");
console.log("path keys", Object.getOwnPropertyNames(path).sort().join(","));
for (const name of ["dirname", "relative", "basename", "extname", "normalize", "isAbsolute",
                    "join", "resolve", "sep"]) {
  console.log("path." + name, typeof path[name]);
}
for (const name of ["existsSync", "readFileSync", "realpathSync", "promises"]) {
  console.log("fs." + name, typeof fs[name]);
}
const { dirname, relative, sep } = require("path");
console.log("destructured", typeof dirname, typeof relative, typeof sep);
try {
  console.log("dirname call", dirname("a/b/c.css"));
} catch (error) {
  console.log("dirname call THREW", error.name + ": " + error.message);
}
