// An object literal's `DefineNamedProperty` remembers the shape transition it took at each
// site. The same site must answer correctly when the incoming shape differs (a spread of
// objects with different shapes), when the key already exists on the target (a spread that
// carried it), when the target is frozen, in dictionary mode, or has an accessor, when the
// value is a function (which takes its name from the key), and for `__proto__`.
const shapes = [
  { a: 1 }, { b: 2 }, { a: 1, b: 2 }, { c: 3, a: 1 }, { x: 1, y: 2, z: 3, w: 4, v: 5, u: 6, t: 7, s: 8, r: 9 },
];
function build(src, i) { return { ...src, k: i, a: "over" }; }
const out = [];
for (let round = 0; round < 3; round++) {
  for (let i = 0; i < shapes.length; i++) {
    const o = build(shapes[i], i);
    out.push(Object.keys(o).join("") + ":" + o.k + ":" + o.a);
  }
}
console.log(out.join(" "));

function withFn(src) { return { ...src, f() { return 1; }, g: function () {}, h: () => 2 }; }
const wf = withFn({ q: 1 });
console.log(Object.keys(wf).join(","), wf.f.name, wf.g.name, wf.h.name, wf.f(), wf.h());

function protoKey(p) { return { __proto__: p, own: 1 }; }
const pk = protoKey({ inherited: 7 });
console.log(Object.keys(pk).join(","), pk.inherited, Object.getPrototypeOf(pk).inherited);

// The same literal site sees a dictionary-mode target through a spread of a large object.
const big = {};
for (let i = 0; i < 40; i++) big["p" + i] = i;
function spreadBig(src) { return { ...src, tail: true }; }
const sb = spreadBig(big);
console.log(Object.keys(sb).length, sb.p39, sb.tail);
const sb2 = spreadBig({ small: 1 });
console.log(Object.keys(sb2).join(","), sb2.tail);

// Repeated construction at one site keeps values distinct and shapes independent.
function point(x, y) { return { x, y }; }
const pts = [];
for (let i = 0; i < 5; i++) pts.push(point(i, i * i));
pts[2].extra = "e";
console.log(pts.map((p) => p.x + "/" + p.y + "/" + (p.extra || "-")).join(" "), Object.keys(pts[3]).join(""));

console.log(JSON.stringify({ ...Object.freeze({ z: 1 }), z: 2, y: 3 }));
