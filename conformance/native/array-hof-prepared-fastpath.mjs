// Array higher-order methods at a call site where the method is read before the arguments are
// evaluated. The loops lowered at the site must agree with the specification loop in every case
// where their guards fail. Node's output is the oracle.
const log = (label, value) => console.log(label, JSON.stringify(value));

// Plain dense receiver: the fast path.
const nums = [];
for (let i = 0; i < 50; i++) nums.push(i);
log("chain", nums.map((x) => x * 2 + 1).filter((x) => x % 3 === 0).reduce((a, x) => a + x, 0));
log("forEach", (() => { let s = 0; nums.forEach((x) => { s += x; }); return s; })());
log("some/every", [nums.some((x) => x > 48), nums.every((x) => x < 50), nums.some((x) => x > 99)]);
log("find", [nums.find((x) => x > 10), nums.findIndex((x) => x > 10), nums.find((x) => x > 99)]);
log("indexOf/includes", [nums.indexOf(7), nums.includes(7), nums.indexOf(77), nums.includes(77)]);
log("-0/NaN", [[0].includes(-0), [-0].indexOf(0), [NaN].includes(NaN), [NaN].indexOf(NaN)]);

// A callback that mutates the array while it is walked.
{
  const a = [1, 2, 3, 4, 5];
  log("push during map", a.map((x, i) => { if (i === 0) a.push(99); return x; }));
  const b = [1, 2, 3, 4, 5];
  log("pop during map", b.map((x, i) => { if (i === 0) b.pop(); return x * 2; }));
  const c = [1, 2, 3, 4, 5];
  log("shrink during forEach", (() => { const seen = []; c.forEach((x, i) => { seen.push(x); if (i === 1) c.length = 2; }); return seen; })());
  const d = [1, 2, 3, 4, 5];
  log("hole during filter", d.filter((x, i) => { if (i === 0) delete d[2]; return true; }));
  const e = [1, 2, 3, 4, 5];
  log("change during reduce", e.reduce((acc, x, i) => { if (i === 1) e[3] = 100; return acc + x; }, 0));
  const f = [1, 2, 3];
  log("some mutate", f.some((x, i) => { if (i === 0) f[1] = 50; return x === 50; }));
}

// Holey and sparse arrays.
log("holey map", [1, , 3].map((x) => x * 2));
log("holey filter", [1, , 3].filter(() => true));
log("holey reduce", [, 1, , 3].reduce((a, x) => a + x));
log("holey indexOf/includes", [[, 1].indexOf(undefined), [, 1].includes(undefined)]);
log("sparse", (() => { const s = []; s[20] = 1; let n = 0; s.forEach(() => n++); return [n, s.length, s.map((x) => x).length]; })());

// An element behind a getter.
{
  const g = [1, 2, 3];
  Object.defineProperty(g, 1, { get() { return 40; }, enumerable: true, configurable: true });
  log("getter element", g.map((x) => x + 1));
}

// Subclass with species.
{
  class Mine extends Array {}
  const m = Mine.from([1, 2, 3]);
  log("species map", [m.map((x) => x) instanceof Mine, m.filter(() => true) instanceof Mine]);
  class Plain extends Array { static get [Symbol.species]() { return Array; } }
  const p = Plain.from([1, 2, 3]);
  log("species Array", [p.map((x) => x) instanceof Plain, p.map((x) => x) instanceof Array]);
}

// thisArg, and a non-callable callback.
log("thisArg", [1, 2].map(function () { return this.k; }, { k: 7 }));
log("no callback", (() => { try { [].map(1); } catch (e) { return e instanceof TypeError; } })());

// reduce with and without an initial value on an empty array.
log("reduce empty init", [].reduce((a, x) => a + x, 5));
log("reduce empty", (() => { try { [].reduce((a, x) => a + x); } catch (e) { return e instanceof TypeError; } })());
log("reduce one", [4].reduce((a, x) => a + x));

// Array-likes and proxies.
log("array-like", Array.prototype.map.call({ length: 2, 0: "a", 1: "b" }, (x) => x + x));
log("string", Array.prototype.map.call("abc", (x) => x.toUpperCase()));
{
  const trail = [];
  const proxy = new Proxy([1, 2, 3], { get(t, k, r) { trail.push(String(k)); return Reflect.get(t, k, r); } });
  log("proxy map", [Array.prototype.map.call(proxy, (x) => x * 2), trail.length > 0]);
}

// An own property on the receiver shadows the built-in.
{
  const own = [1, 2, 3];
  own.map = function () { return "own"; };
  log("own map", own.map((x) => x));
}

// An argument that changes the method between its lookup and the call.
{
  const swap = [1, 2, 3];
  const original = Array.prototype.filter;
  log("arg patches method", swap.filter((x) => { return x > 1; }));
  Array.prototype.filter = function () { return "patched"; };
  log("patched filter", swap.filter((x) => x > 1));
  Array.prototype.filter = original;
}

// A patched Array.prototype.map is honored.
{
  const original = Array.prototype.map;
  Array.prototype.map = function (f) { return ["patched", original.call(this, f).length]; };
  log("patched map", [1, 2, 3].map((x) => x));
  Array.prototype.map = original;
  log("restored map", [1, 2, 3].map((x) => x + 1));
}

// Types that lie: a string receiver and a typed array take their own methods.
log("typed array", Array.from(new Float64Array([1, 2, 3]).map((x) => x * 2)));
log("string indexOf", "abcabc".indexOf("c"));
