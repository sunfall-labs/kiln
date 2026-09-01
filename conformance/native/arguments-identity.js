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
