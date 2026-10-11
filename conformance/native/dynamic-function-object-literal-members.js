// Object-literal members beyond `key: value` and plain methods, written inside dynamic code:
// getters and setters (plain, computed, string and numeric keys), numeric keys, async methods,
// and the names `get`, `set` and `async` used as ordinary keys.
const run = (name, source, ...args) => {
  try { console.log(name, JSON.stringify(Function("a", "b", source)(...args))); }
  catch (error) { console.log(name, "threw", error.name); }
};
run("getter", "var o = { get x() { return 5; } }; return o.x");
run("setter", "var log = []; var o = { set x(v) { log.push(v); } }; o.x = 3; o.x = 4; return log");
run("getter-setter-pair", "var s = 0; var o = { get x() { return s; }, set x(v) { s = v * 2; } }; o.x = 4; return o.x");
run("getter-uses-this", "var o = { n: 3, get double() { return this.n * 2; } }; o.n = 10; return o.double");
run("getter-descriptor", "var o = { get x() { return 1; } }; var d = Object.getOwnPropertyDescriptor(o, 'x'); return [typeof d.get, d.set, d.enumerable, d.configurable]");
run("getter-enumerable-in-keys", "var o = { get x() { return 1; }, y: 2 }; return Object.keys(o)");
run("getter-only-write-ignored", "var o = { get x() { return 1; } }; o.x = 9; return o.x");
run("getter-names", "var o = { get x() { return 1; }, set x(v) {} }; var d = Object.getOwnPropertyDescriptor(o, 'x'); return [d.get.name, d.set.name]");
run("getter-computed-key", "var k = 'dyn'; var o = { get [k]() { return 'c'; } }; return o.dyn");
run("getter-computed-expression", "var o = { get ['a' + 'b']() { return 1; } }; return Object.keys(o)");
run("getter-string-key", "var o = { get 'quoted key'() { return 'sk'; } }; return o['quoted key']");
run("getter-numeric-key", "var o = { get 0() { return 'zero'; } }; return o[0]");
run("setter-numeric-key", "var seen; var o = { set 7(v) { seen = v; } }; o[7] = 'seven'; return seen");
run("getter-symbol-key", "var s = Symbol('k'); var o = { get [s]() { return 'sym'; } }; return o[s]");
run("getter-keyword-name", "var o = { get default() { return 'd'; }, get class() { return 'c'; } }; return [o.default, o.class]");
run("getter-named-get", "var o = { get get() { return 'gg'; } }; return o.get");
run("setter-named-set", "var v; var o = { set set(x) { v = x; } }; o.set = 1; return v");
run("get-as-key", "var o = { get: 1, set: 2 }; return [o.get, o.set]");
run("get-as-method", "var o = { get() { return 'm'; }, set(x) { return x; } }; return [o.get(), o.set(5)]");
run("get-as-shorthand", "var get = 1, set = 2; var o = { get, set }; return o");
run("get-with-trailing-key", "var o = { get a() { return 1; }, b: 2, get c() { return 3; } }; return [o.a, o.b, o.c]");
run("getter-in-inherited", "var p = { get who() { return 'proto'; } }; var o = Object.create(p); return o.who");
run("getter-spread-evaluates", "var o = { n: 0, get x() { return ++this.n; } }; var c = { ...o }; return [c.x, c.n]");
run("getter-nested-literal", "var o = { get inner() { return { get deep() { return 'd'; } }; } }; return o.inner.deep");
run("getter-closure", "var make = (n) => ({ get v() { return n; } }); return [make(1).v, make(2).v]");
run("getter-throws", "var o = { get x() { throw new RangeError('r'); } }; return o.x");
run("getter-params-error", "return { get x(a) { return 1; } }");
run("setter-params-error", "return { set x() { } }");
run("setter-default-param", "var got; var o = { set x(v = 'dflt') { got = v; } }; o.x = undefined; return got");
run("setter-destructured-param", "var got; var o = { set x({ p }) { got = p; } }; o.x = { p: 8 }; return got");

run("numeric-key", "return { 1: 'a', 2: 'b' }");
run("numeric-key-float", "return { 0.5: 'half', 1.25: 'q' }");
run("numeric-key-hex", "return { 0x10: 'sixteen', 0b11: 'three', 0o7: 'seven' }");
run("numeric-key-order", "return Object.keys({ b: 1, 2: 'x', a: 2, 1: 'y' })");
run("numeric-key-exponent", "return Object.keys({ 1e3: 1, 1e21: 2, 5e-7: 3 })");
run("numeric-key-method", "var o = { 3() { return 'three'; } }; return [o[3](), o[3].name]");
run("numeric-key-lookup", "var o = { 10: 'ten' }; return [o[10], o['10'], 10 in o]");
run("numeric-key-getter-name", "var o = { get 5() { return 1; } }; return Object.getOwnPropertyDescriptor(o, '5').get.name");
run("numeric-key-big-number", "return Object.keys({ 123456789012345680000: 1 })");
run("numeric-key-shorthand-error", "return { 1 }");
run("numeric-key-duplicate", "return { 1: 'a', 1: 'b' }");
run("numeric-key-mixed-literal", "return { a: 1, 2: 2, 'c': 3, [4]: 4 }");

run("async-method", "var o = { async m() { return 1; } }; return typeof o.m().then");
run("async-method-resolves", "var o = { async m(x) { return x + 1; } }; var out = []; o.m(1).then((v) => out.push(v)); return typeof out");
run("async-method-name", "var o = { async m() {} }; return o.m.name");
run("async-method-is-method", "var o = { async m() {} }; return o.m.hasOwnProperty('prototype')");
run("async-computed-method", "var k = 'dyn'; var o = { async [k]() { return 1; } }; return typeof o.dyn().then");
run("async-string-method", "var o = { async 'q r'() { return 1; } }; return typeof o['q r']().then");
run("async-numeric-method", "var o = { async 4() { return 1; } }; return typeof o[4]().then");
run("async-as-key", "var o = { async: 1 }; return o.async");
run("async-as-method", "var o = { async() { return 'plain'; } }; return o.async()");
run("async-as-shorthand", "var async = 3; var o = { async }; return o");
run("async-getter-key", "var o = { get async() { return 'ag'; } }; return o.async");
run("async-after-getter", "var o = { get a() { return 1; }, async b() { return 2; } }; return [o.a, typeof o.b().then]");
run("async-method-this", "var o = { v: 4, async m() { return this.v; } }; var p = o.m(); return typeof p.then");
run("async-method-throws-rejects", "var o = { async m() { throw new Error('x'); } }; var p = o.m(); p.catch(() => {}); return typeof p.then");

run("object-method-still-works", "var o = { m(x, y) { return x + y; } }; return o.m(1, 2)");
run("object-method-shorthand-mix", "var m = 1; var o = { m, n() { return m; }, ['o' + 1]: 3 }; return [o.m, o.n(), o.o1]");
run("object-spread-mix", "var s = { a: 1 }; return { ...s, get b() { return 2; }, 3: 'three' }");
run("object-trailing-comma", "return { get a() { return 1; }, }");
console.log("eval-getter", (0, eval)("({ get x() { return 'ev'; } }).x"));
console.log("eval-numeric", JSON.stringify((0, eval)("({ 1: 'a', 0.5: 'b' })")));
