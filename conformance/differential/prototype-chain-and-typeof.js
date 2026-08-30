// `typeof` on a name belonging to an *enclosing function* has to see it. Every CommonJS shim a
// bundler emits writes `typeof exports == "object"` inside a nested function.
function shim(exports, module) {
  const local = 1;
  return (function () {
    return [typeof exports, typeof module, typeof local, typeof nothingAnywhere].join(",");
  })();
}
console.log(shim({}, {}));
console.log(typeof undeclaredEverywhere, typeof shim);

// The prototype chain has an end, and walking it terminates.
function walk(start) {
  const seen = [];
  let step = start;
  while (step && seen.length < 8) {
    seen.push(
      step === Array.prototype
        ? "Array"
        : step === Object.prototype
          ? "Object"
          : step === Function.prototype
            ? "Function"
            : "?",
    );
    step = Object.getPrototypeOf(step);
  }
  return seen.join(" -> ");
}
console.log(walk(Object.getPrototypeOf([])));
console.log(walk(Object.getPrototypeOf(function () {})));
console.log(Object.getPrototypeOf(Object.prototype), Object.getPrototypeOf({}) === Object.prototype);

// A function's own properties are its assigned ones, which is how the interop shims copy a
// module whose exports are a function.
function moduleExports() {}
moduleExports.chunk = () => 1;
moduleExports.map = () => 2;
const names = Object.getOwnPropertyNames(moduleExports);
console.log(names.includes("chunk"), names.includes("map"));

// A function inherits Object.prototype's methods, which the plain-object check reads off the
// *constructor* rather than off the prototype.
console.log(typeof Object.hasOwnProperty, Object.hasOwnProperty.call({ a: 1 }, "a"));
console.log(typeof global, global === globalThis, global.Object === Object);

// `Object.create(null)` records the absence of a prototype, which is not the same as recording
// nothing: an ordinary object has no link and inherits `Object.prototype`.
const bare = Object.create(null);
console.log(Object.getPrototypeOf(bare), typeof bare.hasOwnProperty);
console.log(Object.getPrototypeOf({}) === Object.prototype, typeof ({}).hasOwnProperty);
