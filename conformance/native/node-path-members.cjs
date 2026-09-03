// `node:path`'s decompositions, and what a member with nothing behind it reads as.
//
// A `.cjs` on purpose: `require` is the half of the loader that hands back a namespace *object*,
// and it is the half every esbuild bundle goes through. The other half -- a static
// `import { dirname } from "node:path"` -- is checked before emission and refuses by name there,
// so it was never the one that could give a wrong answer.
//
// The arithmetic below is the interpreter's, which is this corpus's oracle, and it is not always
// node's. Three places where they part company are exercised deliberately, because a probe that
// avoided them would leave the disagreement to be found in a bundle instead:
//
//   * `dirname("a/.")` is "." here and "a" in node -- `Path::components` normalizes a trailing
//     `.` away, so the last component is `a` and its parent is empty.
//   * `basename("a/..")` is "" here and ".." in node -- `..` is a component, not a name.
//   * `join("", "a")` is "/a" here and "a" in node -- the arguments are joined with a separator
//     before they are normalized, so a leading empty one makes the result absolute.
//
// The second half is the part that matters more. `parse`, `format` and `normalize` are declared
// by the shared ABI and this backend has not built them, and each *reads as a function* rather
// than as `undefined` -- the same word the interpreter answers. That is the whole point: a name
// that reads `undefined` fails a feature test in silence, and `postcss` walked past its own
// `resolve && isAbsolute` guard on the strength of one and died three frames later on "a
// undefined is not a function", with nothing anywhere naming `dirname`. Calling one refuses, and
// `node-path-unbuilt-members-refuse.cjs` is that half; a refusal ends the process, so it cannot
// be this file.
//
// `path.posix` is deliberately not touched here. It is the one published member that is not a
// function in the interpreter -- it is the module itself -- so it refuses on the *read*, and
// reading it here would end this probe. See the companion.
const path = require("path");
const fs = require("fs");

const paths = [
  "a/b/c.css", "/a/b/c.css", "a", "/", "", ".", "..", "./a", "a/", "a/b/",
  "a/.", "a/..", "a/../b", ".hidden", "a.", "a.b.c", "/a", "//a", "a//b",
  "./", "../a", "/a/b/", "x.js", "/.", "/..", "a/b/..",
];
for (const p of paths) {
  console.log(
    JSON.stringify(p),
    "dir", JSON.stringify(path.dirname(p)),
    "base", JSON.stringify(path.basename(p)),
    "ext", JSON.stringify(path.extname(p)),
  );
}

console.log("suffix", JSON.stringify(path.basename("a/b/c.css", ".css")));
console.log("suffix-whole", JSON.stringify(path.basename("a/c.css", "c.css")));
console.log("suffix-nomatch", JSON.stringify(path.basename("a/c.css", ".js")));
console.log("suffix-empty", JSON.stringify(path.basename("a/c.css", "")));

const walks = [
  ["/a/b", "/a/b/c"], ["/a/b/c", "/a/b"], ["/a/b", "/a/b"], ["/a/b/c", "/a/x/y"],
  ["a/b", "c/d"], ["", ""], ["/", "/a"], ["a", ""], ["/a/b/", "/a/b/c/"],
  ["/a/b", "/a/b/c/d/e"], ["/x/y/z", "/"],
];
for (const [from, to] of walks) {
  console.log("relative", JSON.stringify(from), JSON.stringify(to),
    JSON.stringify(path.relative(from, to)));
}

// The trailing separator these two keep is the divergence this branch closed: the native
// normalizer used to drop it, so `join("a", "b/")` answered "a/b" where the interpreter answers
// "a/b/". Nothing reported it, because both engines answered a plausible path.
const joins = [["a", "b/"], ["a", ""], ["", "a"], [], ["a", "..", "b"], ["/a", "b/"],
               ["a", ".", "b"], ["a/", "/b"], ["..", "a"]];
for (const parts of joins) {
  console.log("join", JSON.stringify(parts), JSON.stringify(path.join(...parts)));
}
const resolutions = [["/x", "y/"], ["a"], [], ["/a", "/b"], ["a", "b/"], ["/a", "../b"]];
for (const parts of resolutions) {
  console.log("resolve", JSON.stringify(parts), JSON.stringify(path.resolve(...parts)));
}

console.log("isAbsolute", path.isAbsolute("/a"), path.isAbsolute("a"), path.isAbsolute(""));
console.log("sep", JSON.stringify(path.sep), "delimiter", JSON.stringify(path.delimiter));

// A published name with nothing behind it is a function, not `undefined`. Both engines say so.
for (const name of ["dirname", "relative", "basename", "extname", "isAbsolute", "join",
                    "resolve", "normalize", "parse", "format"]) {
  console.log("typeof path." + name, typeof path[name]);
}
for (const name of ["existsSync", "readFileSync", "realpathSync", "promises"]) {
  console.log("typeof fs." + name, typeof fs[name]);
}
// Destructuring reads every name it mentions whether or not the program calls one, which is why
// the refusal cannot live on the read: this line is what `postcss` opens `previous-map.js` with.
const { existsSync, readFileSync, realpathSync } = require("fs");
const { dirname, isAbsolute, join, relative, sep } = require("path");
console.log("destructured fs", typeof existsSync, typeof readFileSync, typeof realpathSync);
console.log("destructured path", typeof dirname, typeof isAbsolute, typeof join,
  typeof relative, typeof sep);
console.log("destructured dirname works", JSON.stringify(dirname("a/b/c.css")));

// `String.fromCharCode`, which was the third thing `postcss` needed and is not a path member at
// all. The narrowing is the interpreter's saturating `as u16` rather than the specification's
// wrapping `ToUint16`, so both engines answer U+FFFF for 65601 where node answers "A". A shared
// deviation rather than a disagreement, which is the property this corpus measures.
console.log("fromCharCode", JSON.stringify(String.fromCharCode(72, 105, 33)));
console.log("fromCharCode empty", JSON.stringify(String.fromCharCode()));
console.log("fromCharCode astral", JSON.stringify(String.fromCharCode(0xd83d, 0xde00)));
console.log("fromCharCode lone surrogate", JSON.stringify(String.fromCharCode(0xd800)));
console.log("fromCharCode wide", JSON.stringify(String.fromCharCode(0x4e2d, 0xe9)));
console.log("fromCharCode zero", JSON.stringify(String.fromCharCode(0)));
console.log("fromCharCode out of range", String.fromCharCode(65601).charCodeAt(0));
console.log("fromCharCode negative", String.fromCharCode(-1).charCodeAt(0));
console.log("fromCharCode nan", String.fromCharCode(NaN).charCodeAt(0));
console.log("fromCharCode typeof", typeof String.fromCharCode);

// Enumeration is the other half of publishing a name: every name the shared ABI declares is
// really there, whether or not this backend can answer it.
//
// Two names are left out of the comparison, and both are differences of *shape* that this branch
// did not introduce and does not claim to have fixed. Naming them here is the point -- a probe
// that quietly printed around them would leave them to be found in a bundle.
//
//   `default` -- the native CommonJS projection publishes the ESM default export as a key on the
//   namespace object, so `Object.keys(require("path"))` contains "default" natively and does not
//   interpreted (nor in node). Measured, on both engines. Pre-existing, and removing it would
//   change what `require(...).default` answers for every interop shim in every bundle, which is
//   not a thing to do in passing.
//
//   `posix` -- published and refusing on read, so it is deliberately not enumerable: a spread or
//   a `JSON.stringify` reads every enumerable value, and refusing the whole of those over one
//   member nobody asked for trades a wrong answer for a worse one. The interpreter enumerates it,
//   because it has one to give. See `node-path-unbuilt-members-refuse.cjs`.
const shapeDifferences = new Set(["default", "posix"]);
const named = (list) => list.filter((key) => !shapeDifferences.has(key)).sort().join(",");
console.log("names", named(Object.getOwnPropertyNames(path)));
console.log("keys", named(Object.keys(path)));
console.log("fs keys", named(Object.keys(fs)));
console.log("spread", named(Object.keys({ ...path })));
