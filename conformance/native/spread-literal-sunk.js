// A literal with one spread and constant keys that never escapes is sunk by the fast clone
// under a guard on its source: its fields live in scalars, the source's slots are copied by
// learnt slot index, and a failed guard or a non-number field read rebuilds the literal for the
// generic clone. Every path must answer as the allocation would, and the same file runs under
// KILN_SPECULATE=exit, where every guard fails.
const base = { a: 1, b: "hello", c: true, d: [1, 2], e: 42 };

// The benchmark's shape: numbers read out of a spread source and a define.
function sumFields() {
  let count = 0;
  for (let i = 0; i < 1000; i++) {
    const merged = { ...base, f: i, g: i * 2 };
    count += merged.f + merged.a + merged.g;
  }
  return count;
}
console.log("sum", sumFields());

// A snapshot: the source changes after the spread, the literal keeps what it copied.
function snapshot(src) {
  const m = { ...src, k: 1 };
  src.a = "changed";
  const seen = m.a + m.k;
  src.a = 1;
  return seen;
}
console.log("snapshot", snapshot(base), snapshot(base));

// The source's value changes between iterations with the shape unchanged.
function tracking() {
  const src = { n: 0 };
  let total = 0;
  for (let i = 0; i < 50; i++) {
    src.n = i;
    const m = { ...src, extra: 1 };
    total += m.n * 10 + m.extra;
  }
  return total;
}
console.log("tracking", tracking());

// The source's shape changes: a slot moves, a key disappears, a key appears.
function relearn(sources) {
  const out = [];
  for (const src of sources) {
    const m = { ...src, z: 7 };
    out.push(String(m.p) + "/" + String(m.q) + "/" + m.z);
  }
  return out.join(" ");
}
console.log("relearn", relearn([{ p: 1, q: 2 }, { q: 3, p: 4 }, { p: 5 }, { q: 6 }, {}, { p: 8, q: 9, r: 10 }]));

// Sources the guard refuses: an accessor, a dictionary, a symbol key, an array, a string,
// null and undefined, an object with index keys, a proxy, a frozen object, an inherited key.
const big = {};
for (let i = 0; i < 40; i++) big["p" + i] = i;
const sym = Symbol("s");
function refused(src) {
  const m = { ...src, own: "o" };
  return String(m.a) + "/" + String(m.p39) + "/" + String(m[0]) + "/" + m.own + "/" + String(m.inh) + "/" + String(m.g);
}
console.log("getter", refused({ get a() { return "got"; } }));
console.log("dictionary", refused(big));
console.log("symbol", refused({ [sym]: 1, a: "sk" }));
console.log("array", refused([9, 8]));
console.log("string", refused("xy"));
console.log("null", refused(null), refused(undefined));
console.log("index", refused({ 0: "zero", a: "ia" }));
console.log("proxy", refused(new Proxy({ a: "pa" }, {})));
console.log("frozen", refused(Object.freeze({ a: "fa" })));
console.log("inherited", refused(Object.assign(Object.create({ inh: "no" }), { a: "own" })));
console.log("number", refused(5), refused(true));

// A key before the spread is overwritten by the source; one after overwrites it.
function order(src) {
  const before = { a: "before", ...src };
  const after = { ...src, a: "after" };
  return before.a + " " + after.a + " " + before.b + " " + after.b;
}
console.log("order", order(base), order({ b: "only" }));

// A field read that feeds arithmetic and is not a number: a string, an absent key, an object,
// and a source whose field changes type mid-loop.
function notNumber() {
  const out = [];
  out.push(({ ...base, k: 1 }).b + 1);
  out.push(({ ...base, k: 1 }).missing + 1);
  const m = { ...base, k: 1 };
  out.push(m.d + 1);
  const src = { v: 1 };
  let total = 0;
  for (let i = 0; i < 30; i++) {
    if (i === 20) src.v = "s";
    const n = { ...src, w: i };
    total = total + n.v + n.w;
  }
  out.push(total);
  return out.join(" ");
}
console.log("notNumber", notNumber());

// A sunk literal written after construction, and read; a conditional define value.
function written(flag) {
  const m = { ...base, f: flag ? 1 : "one" };
  m.a = m.a + 10;
  m.h = m.f;
  return m.a + " " + m.h + " " + m.e;
}
console.log("written", written(true), written(false));

// Two sunk literals in one function, one built from the other's source, both read after a
// method call whose answer feeds arithmetic (an exit after both were sunk).
function twoLiterals(src, s) {
  const first = { ...src, x: 1 };
  const second = { ...src, y: 2 };
  const code = s.charCodeAt(0) + first.a + second.y + first.x;
  return code + " " + first.a + " " + second.a;
}
console.log("two", twoLiterals(base, "A"), twoLiterals({ a: 100 }, "B"), twoLiterals({ a: "s" }, "C"));

// Literals that escape or spread twice are allocated: returned, passed, spread again, captured,
// read by a prototype name, and enumerated.
function escaping(src) {
  const returned = { ...src, r: 1 };
  const passed = { ...src, p: 2 };
  const twice = { ...src, ...{ t: 3 }, u: 4 };
  const captured = { ...src, c: 5 };
  const named = { ...src, n: 6 };
  const enumerated = { ...src, e: 7 };
  const f = () => captured.c;
  return [
    Object.keys(returned).join(","),
    JSON.stringify(passed),
    Object.keys(twice).join(",") + twice.t + twice.u,
    f(),
    typeof named.toString,
    Object.keys(enumerated).length,
  ].join(" | ");
}
console.log("escaping", escaping(base));

// A sunk literal read inside a nested loop, and one alive across a loop with an exit in it.
function acrossLoop(src, s) {
  const m = { ...src, k: 3 };
  let total = 0;
  for (let i = 0; i < 20; i++) {
    total += m.a + m.k + (i === 10 ? s.charCodeAt(0) : 0);
  }
  return total;
}
console.log("across", acrossLoop(base, "z"), acrossLoop({ a: 2 }, "y"), acrossLoop({ a: "q" }, "x"));
