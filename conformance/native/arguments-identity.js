// `arguments` names one object per call, not a snapshot of the list. Both engines materialised a
// fresh array on every mention, so a write through it went somewhere nothing else could read and
// `arguments === arguments` was false. Sloppy or strict makes no difference to any of this: the
// mapping between `arguments[0]` and a parameter is what strict mode removes, and neither engine
// ever had that.
function ownIdentity() {
  return arguments === arguments;
}
console.log("identity", ownIdentity(1));

function writeThrough() {
  arguments[0] = 9;
  return arguments[0];
}
console.log("write", writeThrough(1));

function definedOnIt() {
  Object.defineProperty(arguments, "0", { value: 10, writable: true, enumerable: true, configurable: true });
  return arguments[0];
}
console.log("defined", definedOnIt(0, 1));

// An arrow has no `arguments` of its own: it sees the enclosing call's *object*, so a write
// through the arrow is a write the enclosing function reads back.
function sharedWithArrow() {
  const read = () => arguments;
  const write = () => { arguments[0] = 7; };
  write();
  return [read() === arguments, read() === read(), arguments[0]].join(",");
}
console.log("arrow", sharedWithArrow(1, 2));

function nestedArrow() {
  const outer = () => () => arguments;
  return outer()() === arguments;
}
console.log("nested", nestedArrow(1));

// Two calls of the same function have two objects; a frame restores its own on the way back.
function perCall(depth) {
  if (depth === 0) {
    return arguments;
  }
  const inner = perCall(0);
  return inner !== arguments && arguments[0] === depth;
}
console.log("per-call", perCall(1));

// `arguments` is an *object*, not an array, and the difference is what four separate type tests
// ask about. Both engines used to hand back an array: `Array.isArray(arguments)` said true,
// `Object.prototype.toString.call(arguments)` said `[object Array]`, the prototype was
// `Array.prototype`, and `length` was the array's own rather than an ordinary property. The
// `@@iterator` here is an own property for exactly that reason -- `Object.prototype` has none,
// so a spread would stop working the moment this stopped being an array.
function shape(a, b) {
  return [
    Object.prototype.toString.call(arguments),
    Array.isArray(arguments),
    Object.getPrototypeOf(arguments) === Object.getPrototypeOf({}),
    Object.getOwnPropertyNames(arguments).join("|"),
    Object.keys(arguments).join("|"),
    JSON.stringify(Object.getOwnPropertyDescriptor(arguments, "length")),
    Object.getOwnPropertyDescriptor(arguments, Symbol.iterator).value === [][Symbol.iterator],
    [...arguments].join("|"),
    Array.prototype.slice.call(arguments).join("|"),
  ].join(" ");
}
console.log("shape", shape(1, "two"));

// `length` is an ordinary writable property that nothing keeps in step with the indices, which
// is the whole difference from an array's.
function lengthIsOrdinary() {
  arguments.length = 5;
  arguments[9] = "nine";
  return [arguments.length, Object.getOwnPropertyNames(arguments).join("|")].join(" ");
}
console.log("length", lengthIsOrdinary(1));

// This module is strict, so its `arguments` is the *unmapped* object, whose `callee` is a pair of
// `%ThrowTypeError%` rather than the function. (Running this file under `node` directly answers
// "function" instead, because node reads a bare `.js` as CommonJS and kiln reads it as ESM. The
// comparison here is kiln against kiln, so the goal is the same on both sides; the mapped object
// is gated by `arguments-mapped.cjs`, which is the corpus's one sloppy entry.)
function unmappedCallee() {
  try {
    return typeof arguments.callee;
  } catch (error) {
    return error.constructor.name;
  }
}
console.log("callee", unmappedCallee());
