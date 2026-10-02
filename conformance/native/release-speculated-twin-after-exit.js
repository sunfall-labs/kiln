// kiln-native: profile=native-release
//
// A versioned (release) body runs a fast clone that speculates and a generic clone it exits
// into mid-body, and the two share every variable. `1 + needle.length` is a number only by
// speculation (`needle.length` could be anything), so the generic clone computes it boxed; a
// call argument there once read its unboxed twin instead, which only the fast clone writes,
// and after `kept += list.slice(...)` exits the fast clone (a string is not the number the
// `+=` speculated) the twin was still 0.0. `indexOf("x", 3)` became `indexOf("x", 0)`.
// In `web.ts`'s `headerListRemove` that made `Headers.set`/`delete` loop forever
// (docs/issues/parallel-builtins issue 18). The loops below carry a guard so that a regression
// prints instead of hanging.

function straight(list, needle) {
  let kept = "";
  kept += list.slice(0, 1);
  return kept + list.indexOf("x", 1 + needle.length);
}
console.log(straight("axbxcx", "ab"));

function headerListRemove(list, needle) {
  let at = list.indexOf(needle);
  if (at < 0) return list;
  let kept = "";
  let from = 0;
  let guard = 0;
  while (at >= 0) {
    if (++guard > 50) return "stuck: from=" + from + " at=" + at;
    kept += list.slice(from, at);
    const end = list.indexOf("\n", at + needle.length);
    if (end < 0) return kept;
    from = end;
    at = list.indexOf(needle, from);
  }
  return kept + list.slice(from);
}
console.log(JSON.stringify(headerListRemove("\nx-dup:1\ncontent-type:text/html\nx-dup:2\nz:1", "\nx-dup:")));
console.log(JSON.stringify(headerListRemove("\na:1\nx-dup:2", "\nx-dup:")));
console.log(JSON.stringify(headerListRemove("\na:1", "\nx-dup:")));

// The same shape where the speculation holds on one call and fails on the next: a `length`
// that is not a number reaches the generic clone's boxed sum, and the call must see it.
function offsetOf(text, marker, from) {
  let out = "";
  out += text.slice(0, 1);
  return out + ":" + text.indexOf("|", from + marker.length);
}
console.log(offsetOf("a|b|c|d", "xy", 0), offsetOf("a|b|c|d", { length: "1" }, 0), offsetOf("a|b|c|d", { length: 4 }, 0));

// A speculated product computed *before* the exit and read after it: the generic clone reads it
// boxed, and the fast clone, which never needed the box, writes it on the way out.
function before(o, s) {
  const n = o.a * 2;
  let k = "";
  k += s.slice(0, 1);
  return s.indexOf("x", n) + k.length + ":" + n;
}
console.log(before({ a: 1 }, "axbxcx"), before({ a: 2 }, "axbxcxdx"), before({ a: "1" }, "axbxcx"));
function beforeArith(o, s) {
  const n = o.a * 4;
  let k = "";
  k += s.slice(0, 1);
  return n * 2 + k.length;
}
console.log(beforeArith({ a: 1 }, "axbx"), beforeArith({ a: 0.5 }, "axbx"), beforeArith({ a: "2" }, "axbx"));

// A speculated sum consumed by a property write and by a phi after the exit.
function carry(values, key) {
  const box = {};
  let acc = "";
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    acc += values[i].slice(0, 1);
    total = total + key.length;
    box[i] = total;
  }
  return acc + ":" + JSON.stringify(box) + ":" + total;
}
console.log(carry(["ab", "cd", "ef"], "xyz"), carry(["ab"], { length: "7" }));
