// A callsite's method alias belongs to the suspended caller. Nested calls that return or throw
// must not replace it before an Error captures the outer frame.
Error.prepareStackTrace = (_error, sites) => sites.map((site) => site.getMethodName() || "").join("\n");

function capturedMethodAlias(key, nestedWork) {
  const callback = () => {
    nestedWork();
    const throwAfterNestedCall = () => { throw new Error("expected"); };
    throwAfterNestedCall();
  };
  // Keep the closure anonymous so the stack must use its computed callsite method name.
  Object.defineProperty(callback, "name", { value: "", configurable: true });
  const receiver = { [key]: callback };
  try {
    receiver[key]();
  } catch (error) {
    return String(error.stack).split("\n").some((name) => name === key);
  }
  return false;
}

const afterReturn = capturedMethodAlias("~probe/afterReturn", () => {
  [1].find((value) => value === 1);
});
const afterCaughtThrow = capturedMethodAlias("~probe/afterCaughtThrow", () => {
  try {
    const throwInsideNestedCall = () => { throw new Error("caught"); };
    throwInsideNestedCall();
  } catch (_error) {
    // The caller keeps its own alias when the nested frame is unwound.
  }
});
if (!afterReturn || !afterCaughtThrow) {
  throw new Error(`callsite method name was lost: return=${afterReturn}, caught=${afterCaughtThrow}`);
}
console.log("nested callsite aliases restored");
