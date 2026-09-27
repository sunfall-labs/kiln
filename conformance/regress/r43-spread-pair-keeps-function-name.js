// Issue 28, item 2: a pair written after a spread is defined on the literal directly, and that
// define named its function value after the key even when the function already had a name --
// `{ ...{}, f: foo }` renamed `foo` to "f" in both engines. Only an anonymous function
// definition takes the property's name. Recorded from node 26.8.1.
function foo() {}
const o = { ...{}, f: foo };
console.log(foo.name, o.f.name, o.f === foo);
const bar = () => {};
const p = { ...{ a: 1 }, [("b" + "c")]: bar, d: foo };
console.log(bar.name, p.bc.name, p.d.name, Object.keys(p).join(","));
class K {}
const q = { ...{}, k: K };
console.log(K.name, q.k.name);
// An anonymous one still takes the key, after a spread as anywhere else.
const r = { ...{}, g: function () {}, h: () => {}, [("i")]: class {}, j() {}, [("m")]() {} };
console.log(r.g.name, r.h.name, r.i.name, r.j.name, r.m.name);
// A value that is not a function is defined as it is, and no setter runs.
const u = { ...{}, x: 1, ["__proto__"]: 2 };
console.log(u.x, Object.getOwnPropertyNames(u).join(","), Object.getPrototypeOf(u) === Object.prototype);
// A bound function keeps its own name too.
const bound = foo.bind(null);
console.log({ ...{}, w: bound }.w.name);
