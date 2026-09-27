// Issue 24: a computed key is converted with `ToPropertyKey` where it is written -- after the key
// and before the value, so its `toString` (or `@@toPrimitive`) runs in source order and a throw
// from it stops the literal there. The native backend named the property "[object Object]" and
// called nothing; the interpreter converted it only after every value had run. Object literals,
// accessors, class methods and static fields. Recorded from node 26.8.1.
const seen = [];
const k = { toString() { seen.push("k"); return "kk"; } };
function g() { const o = { [k]: 1 }; return Object.keys(o); }
console.log(g().join(), seen.join());
class C { [k]() { return 1; } static [k] = 2; get [k + "g"]() { return 3; } }
console.log(Object.getOwnPropertyNames(C.prototype).join(), typeof C.kk, new C().kk(), seen.join());
const tp = { [Symbol.toPrimitive](h) { seen.push(h); return "tp"; } };
const o2 = { a: null, [tp]: 1, [tp]() {}, get [tp]() { return 5; } };
console.log(Object.keys(o2).join(), o2.tp, seen.join());
const vo = { valueOf() { return 1; }, toString: null };
console.log(Object.keys({ [vo]: 1 }).join());
const bad = { toString() { throw new RangeError("key"); } };
try { const o3 = { [bad]: 1 }; console.log("no throw", Object.keys(o3)); } catch (e) { console.log("caught", e.constructor.name, e.message); }
try { class D { [bad]() {} } console.log("no throw"); } catch (e) { console.log("caught", e.constructor.name, e.message); }
try { class D { static [bad] = 1; } console.log("no throw"); } catch (e) { console.log("caught", e.constructor.name, e.message); }
const order = [];
const ko = (name) => ({ toString() { order.push("key " + name); return name; } });
const val = (name) => (order.push("value " + name), name);
const r = { [ko("a")]: val(1), b: val(2), [ko("c")]: val(3), ...{}, [ko("d")]: val(4), get [ko("e")]() { return 5; } };
console.log(order.join(), JSON.stringify(r));
order.length = 0;
class E { [ko("m")]() {} static [ko("s")]() {} get [ko("g")]() { return 1; } }
console.log(order.join());
const s = Symbol("sym");
const sk = { [Symbol.toPrimitive]() { return s; } };
const withSymbol = { [sk]: 9 };
console.log(withSymbol[s], Object.getOwnPropertySymbols(withSymbol).length);
const named = { [k]: function () {}, [sk]: () => {} };
console.log(named.kk.name, named[s].name);
