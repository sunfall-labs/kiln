// Two shapes the SSA graph emitter got wrong on 2026-09-05, both places the stack emitter had a
// special case and the graph emitter used the generic path. mustache's `hasProperty` printed
// `Hello !`; postcss and vue-reactivity segfaulted.

// `in` and `instanceof` are not arithmetic. Handed to the generic binary helper they answered
// something else; they are lowered inline against their own prelude entry points.
function hasProperty(obj, propName) {
  return obj != null && typeof obj === "object" && propName in obj;
}
function isDate(v) {
  return v instanceof Date;
}
console.log(hasProperty({ name: "kiln" }, "name"), hasProperty({}, "name"), hasProperty(null, "x"));
console.log(isDate(new Date(0)), isDate({}));
let threw = "no";
try { hasProperty(5, "x"); "x" in 5; } catch (e) { threw = e.constructor.name; }
console.log(threw);

// A block-scoped `let` captured by a closure lives in a cell, and re-entering its block marks the
// cell's contents uninitialized. Marking the slot instead dropped the cell pointer, and the next
// write through the cell dereferenced the sentinel.
function collect(n) {
  const fns = [];
  for (let i = 0; i < n; i++) {
    let seen = i * 10;
    fns.push(() => seen);
    seen += 1;
  }
  let k = 0;
  while (k < n) {
    let inner = k;
    fns.push(() => inner);
    k += 1;
  }
  return fns.map((f) => f()).join(",");
}
console.log(collect(3));
