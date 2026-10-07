// An object literal whose property value is a call, stored through a computed key on `this`.
// full-runtime issue 07: the URL constructor was hoisted (`const rec = parseRecord(...)`) because
// `{ c: parseRecord(input, base), ... }` was reported to read `c` back as `undefined` when the
// bootstrap is emitted generic-only. The literal and the hoisted form must agree, natively and
// in the interpreter, including the real URL class that uses the literal form.
function parse(input, base) {
  return String(input).split("/").concat(base === undefined ? [] : [String(base)]);
}
const STATE = Symbol("state");
class Holder {
  constructor(input, base) {
    this[STATE] = { c: parse(input, base), sp: undefined, input: undefined, base: undefined };
  }
  get first() { return this[STATE].c[0]; }
  get count() { return this[STATE].c.length; }
  get untouched() { return this[STATE].sp; }
}
class Hoisted {
  constructor(input, base) {
    const rec = parse(input, base);
    this[STATE] = { c: rec, sp: undefined, input: undefined, base: undefined };
  }
  get first() { return this[STATE].c[0]; }
  get count() { return this[STATE].c.length; }
}
const keep = [];
for (let i = 0; i < 200; i++) {
  const a = new Holder("a/b/" + i, i % 3 === 0 ? "base" : undefined);
  const b = new Hoisted("a/b/" + i, i % 3 === 0 ? "base" : undefined);
  keep.push({ x: [i, i + 1], y: "s" + i });
  if (a.first !== b.first || a.count !== b.count || a.untouched !== undefined) {
    throw new Error("literal call value lost at " + i + ": " + a.first + " " + a.count);
  }
  if (i % 50 === 0) console.log(i, a.first, a.count, a.untouched, JSON.stringify(a[STATE].c));
}
console.log("literal call value", "ok", keep.length);

const u = new URL("https://user:pw@example.com:8080/p/a/t/h?query=string#hash");
console.log(u.href, u.host, u.pathname, u.search, u.hash);
console.log(new URL("../x?y=1", "http://h.test/a/b/c").href);
