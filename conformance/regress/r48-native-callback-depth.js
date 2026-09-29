// Function.prototype.call forwards into the existing VM loop. Nested built-in callbacks still
// re-enter Rust and must throw a catchable RangeError before the host stack is exhausted.
function viaCall(n) {
  return n === 0 ? 0 : 1 + viaCall.call(null, n - 1);
}
console.log("call", viaCall(150));

function viaApply(n) {
  return n === 0 ? 0 : 1 + viaApply.apply(null, [n - 1]);
}
console.log("apply", viaApply(150));

function nested(n) {
  return n === 0 ? 0 : [0].map(() => nested(n - 1))[0] + 1;
}
try {
  nested(5000);
  console.log("nested", false);
} catch (error) {
  console.log("nested", error instanceof RangeError);
}
console.log("after", viaCall(5));
