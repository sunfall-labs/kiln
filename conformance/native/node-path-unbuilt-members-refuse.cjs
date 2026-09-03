// A `node:` member this backend has not built refuses by name when it is called.
//
// This probe is *expected* to appear in the refusal list, and that is what it is for. The
// interpreter answers `path.parse(...)` with an object; the native backend stops and says
// `CallMethod(parse)`. One is a refusal and the other is an answer, so `scripts/native.sh`
// records it as backlog rather than as a divergence -- which is the correct reading, and the
// reason a refusal is allowed at all: it names what is missing instead of guessing at it.
//
// What this is really testing is that the name resolves to something callable. Before this
// branch, `require("path").parse` was `undefined` -- present, enumerable, and answering the one
// wrong answer this backend is not allowed to give -- while a static `import { parse }` of the
// same member refused before emission. The two halves of the loader disagreed, and the half that
// lied is the half every bundle uses.
//
// It has to be its own file because a refusal ends the process: nothing after the call runs, so
// the members that *are* built are measured next door in `node-path-members.cjs`.
//
// The sibling shape is `path.posix`, which refuses on the *read* rather than on a call. It is the
// one published member of `node:path` that is not a function in the interpreter -- it is the
// module itself, so `require("path").posix === require("path")` is true there -- and natively the
// ESM namespace and the CommonJS projection are two different objects, so no value this backend
// can hand back has the right identity. A callable would at least be a lie about `typeof`. That
// refusal is not exercised here for the same reason `parse` needs its own file, and reads:
//
//   kiln: native backend refusal: ... operation=GetProperty(posix) detail=GetProperty lookup refused
const path = require("path");

// Callable, in both engines, rather than `undefined` in one of them. This much runs natively.
console.log("typeof parse", typeof path.parse);
console.log("typeof format", typeof path.format);
console.log("typeof normalize", typeof path.normalize);

// And here the native backend stops, naming the member it was asked for.
console.log("parse", JSON.stringify(path.parse("/a/b/c.css")));
console.log("unreached");
