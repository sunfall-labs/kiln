// Generic String methods use abstract ToString and keep the resulting UTF-16 code units.
const assert = (condition, label) => {
  if (!condition) throw new Error(`String method conversion: ${label}`);
};
const lone = String.fromCharCode(0xd800);
const concat = String.prototype.concat;
const boxed = Object.assign(new String("ignored"), {
  [Symbol.toPrimitive](hint) {
    assert(hint === "string", "boxed generic receiver gets the string hint");
    return lone;
  },
});
assert(concat.call(boxed, "x") === lone + "x", "concat preserves boxed receiver units");
assert(concat.call("", {
  toString() { return lone + "tail"; },
}) === lone + "tail", "concat preserves object argument units");
assert("".padStart(1, {
  toString() { return lone; },
}) === lone, "padStart preserves filler units");
assert("".padEnd(1, {
  [Symbol.toPrimitive](hint) { return hint === "string" ? lone : "wrong"; },
}) === lone, "padEnd preserves ToPrimitive filler units");

let symbolRejected = false;
try { concat.call("", Symbol("text")); }
catch (error) { symbolRejected = error instanceof TypeError; }
assert(symbolRejected, "concat rejects Symbol using ToString semantics");

// Repeated fresh strings and values ensure each conversion is evaluated independently and its
// temporary units are copied before the next user conversion can allocate or collect.
for (let i = 0; i < 80; i++) {
  const actual = concat.call({ toString() { return lone + String(i); } }, {
    [Symbol.toPrimitive](hint) { return hint === "string" ? lone + "!" : "wrong"; },
  });
  assert(actual === lone + String(i) + lone + "!", `temporary converted units ${i}`);
}
console.log("String concat and padding preserve UTF-16 ToString results");
