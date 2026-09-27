// Issue 23: an array's string conversion `ToString`s each element, and for an object that is a
// call -- its own `toString`, `@@toPrimitive` or `valueOf`. `[o] + ""`, a template and `String`
// reach `join` through `Array.prototype.toString`; the interpreter joined the dense array without
// the call there, and the native `join` never made it. Recorded from node 26.8.1.
const seen = [];
const o = { toString() { seen.push("t"); return "y"; } };
console.log([o] + "", `${[o]}`, String([o]), [o].join(), seen.join());
const p = { [Symbol.toPrimitive](hint) { seen.push(hint); return "P"; } };
console.log([1, p, null, undefined, "z"].join("-"), String([p]), seen.join());
const v = { valueOf() { return 7; }, toString: undefined };
console.log([v].join(), `${[v, [v, 2]]}`);
const bad = { toString() { throw new RangeError("nope"); } };
try { console.log([1, bad].join()); } catch (e) { console.log("caught", e.constructor.name, e.message); }
try { console.log(`${[bad]}`); } catch (e) { console.log("caught", e.constructor.name, e.message); }
try { console.log([Symbol("s")].join()); } catch (e) { console.log("caught", e.constructor.name); }
const arr = [1, { toString() { arr.length = 1; return "m"; } }, 3];
console.log(arr.join());
const nested = [[o, 2], o];
console.log(nested + "", seen.length);
