// Object literals with static keys are built straight into the shape their site learned the
// first time, instead of one transition per member. Every literal below runs several times from
// one site, so the later iterations take the remembered shape; the forms the remembered shape
// cannot describe must still come out exactly as the defining path builds them.

function show(label, o) {
  const own = Object.getOwnPropertyNames(o).map((k) => k + "=" + String(o[k]));
  console.log(label, own.join(","), Object.getPrototypeOf(o) === Object.prototype);
}

for (let i = 0; i < 4; i++) {
  // Plain static keys, first keys no bootstrap object uses.
  show("one", { zzOne: i });
  show("three", { zzA: i, zzB: "b" + i, zzC: i % 2 === 0 });
  show("eight", { k1: 1, k2: 2, k3: 3, k4: 4, k5: 5, k6: 6, k7: 7, k8: i });
  // Numbers and non-numbers trading places in one slot across iterations.
  show("mixed", { m: i % 2 === 0 ? i : "s" + i, n: i % 2 === 0 ? "t" : i + 0.5 });
  // A repeated key keeps its first position and its last value.
  show("duplicate", { d1: 1, d2: 2, d1: 3 + i });
  // Index spellings go to the elements, and order integer keys first.
  show("index", { b: 1, 0: "zero", 1: "one" + i, a: 2 });
  show("numeric-looking", { "01": "x", "1.5": "y", "-1": "z", "4294967295": i });
  // `__proto__:` relinks rather than defining a member.
  const proto = { inherited: "yes" + i };
  const linked = { __proto__: proto, own: i };
  console.log("proto", Object.keys(linked).join(","), linked.inherited, Object.getPrototypeOf(linked) === proto);
  // Getters and setters in a literal.
  let stored = 0;
  const acc = { get g() { return "got" + i; }, set s(v) { stored = v; }, plain: i };
  acc.s = i * 10;
  console.log("accessors", acc.g, stored, Object.keys(acc).join(","), acc.plain);
  // Computed keys, a symbol and a spread beside static keys.
  const key = "c" + (i % 2);
  const sym = Symbol.for("lit");
  const computed = { fixed: 1, [key]: i, [sym]: "sym" };
  console.log("computed", Object.keys(computed).join(","), computed[key], computed[sym]);
  const spread = { first: i, ...{ s1: 1, s2: 2 }, last: "L" };
  show("spread", spread);
  // Nested literals, and one written to after it is built.
  const nested = { status: 200 + i, headers: { "content-type": "text/plain", "x-i": String(i) } };
  nested.headers.extra = true;
  nested.status += 1;
  console.log("nested", JSON.stringify(nested));
  // A literal the program then deletes from and extends: its own shape, not the site's.
  const changed = { p: 1, q: 2 };
  delete changed.p;
  changed.r = i;
  show("changed", changed);
  // Wider than the inline slots and wider than the fast-property limit.
  const wide = { w0: 0, w1: 1, w2: 2, w3: 3, w4: 4, w5: 5, w6: 6, w7: 7, w8: 8, w9: 9, w10: 10,
    w11: 11, w12: 12, w13: 13, w14: 14, w15: 15, w16: 16, w17: 17, w18: i };
  console.log("wide", Object.keys(wide).length, wide.w18, wide.w0);
}

// Many distinct first keys: every one of them used to leave the root shape full, after which a
// literal was a dictionary from its first member. Reads, `in`, enumeration and JSON must agree.
const many = [];
for (let i = 0; i < 120; i++) {
  const o = {};
  o["first" + i] = i;
  o.second = i * 2;
  many.push(o);
}
let total = 0;
for (const o of many) total += o.second + o[Object.keys(o)[0]];
console.log("many first keys", total, "first7" in many[7], JSON.stringify(many[119]));
