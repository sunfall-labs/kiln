// The typed callback loop's sloppy-`this` branch: a plain (non-arrow) function entered typed as
// a `map` callback, with no receiver of its own, gets the global object substituted for `this`
// exactly as the boxed path does. `typed-callbacks.js` cannot exercise this half -- it compiles
// as a module, which is strict by definition -- so this is a `.cjs` entry, the only way sloppy
// code reaches this corpus (see the note in `scripts/native.sh`), and it runs with `--node`.
const nums = [1, 2, 3, 4, 5];
console.log(
  "map-this-sloppy",
  nums.map(function sloppyThis(x) {
    return this === globalThis ? "global" : String(this);
  }),
);
