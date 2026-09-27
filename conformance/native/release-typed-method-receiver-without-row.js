// kiln-native: profile=native-release
//
// A typed method entry reads `this` fields through a receiver-shape row, and a row can only be
// claimed by a shape whose fields hold the representation the entry reads (a double here). The
// call site used to enter the typed entry with no row for any receiver of the matched code, and
// the entry, unable to claim one, refused: a class holding a string where it usually holds a
// number stopped the program (issue 21). The site now finds or claims the row *before* it enters
// the entry -- before anything has run -- and a receiver that cannot take one gets the boxed call,
// exactly as a callee without a typed entry would. A throw from inside the entry stays a throw.

class V { constructor(v) { this.v = v; } val() { return this.v; } twice(k) { return this.v * k; } }
const objs = [new V(1), new V("x"), new V(3)];
function sumVals() { let s = 0; for (let i = 0; i < objs.length; i++) s += objs[i].val(); return s; }
console.log(sumVals());
function sumTwice() { let s = 0; for (let i = 0; i < objs.length; i++) s += objs[i].twice(2); return s; }
console.log(sumTwice());
// a non-object receiver through a method value
function readV() { "use strict"; return this.v; }
const holder = { v: 5, readV };
function viaHolder() { let s = 0; for (let i = 0; i < 3; i++) s += holder.readV(); return s; }
console.log(viaHolder());
const vals = [new V(2), new V(4)].map(function (o) { return o.val() * 2; });
console.log(vals);
const mixed = [new V(2), new V("y")].map((o) => o.val() + 1);
console.log(mixed);
// a throw from inside the typed method stays a throw
class T { constructor(v) { this.v = v; } get() { if (this.v > 10) throw new RangeError("big"); return this.v; } }
const ts = [new T(1), new T("s"), new T(20)];
try { let s = 0; for (const t of ts) s += t.get(); console.log(s); } catch (e) { console.log(e.name); }
