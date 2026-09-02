// Per-operation timings for the property paths ADR 0047 touches. Build with `kiln build`
// (wall clock) and run the executable; compare a baseline and an after binary side by side.
const N = 2000000;
function time(name, f) {
  const started = Date.now();
  const r = f();
  console.log(name.padEnd(20), String(Date.now() - started).padStart(6), "ms", r === undefined ? "" : "");
}
class P {
  constructor(x) { this.x = x; this.y = 2; this.z = 3; }
  get() { return this.z; }
}
const o = new P(1);
const proto = { pz: 5 };
const child = Object.create(proto);
child.a = 1; child.b = 2;
time("own read", () => { let s = 0; for (let i = 0; i < N; i++) s += o.z; return s; });
time("own write", () => { for (let i = 0; i < N; i++) o.x = i; });
time("proto read", () => { let s = 0; for (let i = 0; i < N; i++) s += child.pz; return s; });
time("proto method call", () => { let s = 0; for (let i = 0; i < N; i++) s += o.get(); return s; });
time("missing read", () => { let s = 0; for (let i = 0; i < N; i++) s += o.nope === undefined ? 1 : 0; return s; });
time("literal create", () => { let s = 0; for (let i = 0; i < N / 4; i++) { const h = { "content-type": "a", "content-length": "b" }; s += h["content-type"].length; } return s; });
time("computed read", () => { const k = "z"; let s = 0; for (let i = 0; i < N; i++) s += o[k]; return s; });
time("computed write", () => { const k = "y"; for (let i = 0; i < N; i++) o[k] = i; });
time("json parse", () => { let s = 0; for (let i = 0; i < N / 20; i++) s += JSON.parse('{"name":"kiln","values":[1,2,3],"nested":{"a":1,"b":"two"}}').values.length; return s; });
time("json stringify", () => { let s = 0; for (let i = 0; i < N / 20; i++) s += JSON.stringify({ ok: true, served: i, now: 1, items: [1, 2, 3] }).length; return s; });
time("keys", () => { let s = 0; for (let i = 0; i < N / 10; i++) s += Object.keys(o).length; return s; });
time("in", () => { let s = 0; for (let i = 0; i < N; i++) s += ("y" in o) ? 1 : 0; return s; });
time("dict grow", () => { let s = 0; for (let i = 0; i < N / 50; i++) { const d = {}; for (let j = 0; j < 20; j++) d["k" + j] = j; s += d.k7; } return s; });
