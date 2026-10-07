// kiln-native: profile=native-release parallelism=1,2,max
//
// The parallel family is one definition (`kiln-builtins/src/parallel.ts`, ADR 0069) that applies
// the sequential method captured when the realm started. A program that later replaces `map` on
// an instance or on the prototype, or replaces `Function.prototype.call`, changes `map` and leaves
// `parallelMap` alone; each member's `length` and `name` are the sequential method's, and none is
// enumerable. Node has no parallel family, so each line falls back to the sequential method there
// and prints the same text: the comparison is that kiln's family answers exactly that.
const P = typeof [].parallelMap === "function";
const TypedProto = Object.getPrototypeOf(Uint8Array.prototype);
const sequential = { parallelMap: "map", parallelFilter: "filter", parallelFlatMap: "flatMap", parallelSort: "sort" };
function call(receiver, name, args) {
  const owner = Array.isArray(receiver) ? Array.prototype : TypedProto;
  const fn = P ? owner[name] : owner[sequential[name]];
  return Reflect.apply(fn, receiver, args);
}
function attempt(label, f) {
  try { console.log(label, f()); } catch (e) { console.log(label, "threw", e.constructor.name); }
}
const square = (x) => x * x;
const big = [];
for (let i = 0; i < 20000; i++) big.push(i % 101);

// Own properties shadowing the sequential method on the receiver.
const a = [3, 1, 2];
a.map = () => "HIJACKED"; a.filter = () => "HIJACKED"; a.flatMap = () => "HIJACKED"; a.sort = () => "HIJACKED";
const t = new Float64Array([3, 1, 2]);
t.map = () => "HIJACKED"; t.filter = () => "HIJACKED"; t.sort = () => "HIJACKED";
attempt("array own map", () => String(call(a, "parallelMap", [square])));
attempt("array own filter", () => String(call(a, "parallelFilter", [(x) => x > 1])));
attempt("array own flatMap", () => String(call(a, "parallelFlatMap", [(x) => [x, x]])));
attempt("array own sort", () => String(call(a.slice(), "parallelSort", [(x, y) => x - y])));
attempt("typed own map", () => String(call(t, "parallelMap", [square])));
attempt("typed own filter", () => String(call(t, "parallelFilter", [(x) => x > 1])));
attempt("typed own sort", () => String(call(new Float64Array([3, 1, 2]), "parallelSort", [])));

// A receiver's own `parallelMap` is the program's, and wins over every lowering.
const own = big.slice();
own.parallelMap = () => "own";
const ownTyped = new Float64Array(big);
ownTyped.parallelMap = () => "own";
console.log("own member", own.parallelMap(square), ownTyped.parallelMap(square));

// Shape: `length` and `name` are the sequential method's, and nothing is enumerable.
const shapes = [
  ["Array", Array.prototype, ["parallelMap", 1], ["parallelFilter", 1], ["parallelFlatMap", 1], ["parallelSort", 1], ["parallelReduce", 3]],
  ["TypedArray", TypedProto, ["parallelMap", 1], ["parallelFilter", 1], ["parallelSort", 1], ["parallelReduce", 3]],
];
for (const [owner, proto, ...members] of shapes) {
  const line = [];
  for (const [name, length] of members) {
    const d = Object.getOwnPropertyDescriptor(proto, name);
    line.push(P ? `${name}/${d.value.length}/${d.value.name === name}/${d.enumerable}/${d.writable}/${d.configurable}`
      : `${name}/${length}/true/false/true/true`);
  }
  console.log(owner, line.join(" "));
}
const listed = [];
for (const key in []) listed.push(key);
for (const key in new Uint8Array(0)) listed.push(key);
console.log("enumerable", listed.length, Object.keys(Array.prototype).length, Object.keys(TypedProto).length);
console.log("typed flatMap", P ? typeof TypedProto.parallelFlatMap : "undefined");

// The prototype's sequential methods replaced, and `Function.prototype.call` with them.
const saved = {
  map: Array.prototype.map, filter: Array.prototype.filter, flatMap: Array.prototype.flatMap,
  sort: Array.prototype.sort, tmap: TypedProto.map, tfilter: TypedProto.filter,
  tsort: TypedProto.sort, call: Function.prototype.call,
};
const before = {
  map: String(call(big, "parallelMap", [square]).slice(0, 5)),
  tmap: String(call(new Float64Array(big), "parallelMap", [square]).slice(0, 5)),
};
const fresh = {
  map: Array.prototype.map, filter: Array.prototype.filter, flatMap: Array.prototype.flatMap,
  sort: Array.prototype.sort, tmap: TypedProto.map, tfilter: TypedProto.filter, tsort: TypedProto.sort,
};
Array.prototype.map = () => "HIJACKED";
Array.prototype.filter = () => "HIJACKED";
Array.prototype.flatMap = () => "HIJACKED";
Array.prototype.sort = () => "HIJACKED";
TypedProto.map = () => "HIJACKED";
TypedProto.filter = () => "HIJACKED";
TypedProto.sort = () => "HIJACKED";
Function.prototype.call = () => "HIJACKED";
// Node falls back to the methods as they were before the replacement.
function family(receiver, name, args, original) {
  return P ? (Array.isArray(receiver) ? Array.prototype : TypedProto)[name].apply(receiver, args)
    : original.apply(receiver, args);
}
const results = [
  String(family(big, "parallelMap", [square], fresh.map).slice(0, 5)),
  String(family([3, 1, 2], "parallelFilter", [(x) => x > 1], fresh.filter)),
  String(family([1, 2], "parallelFlatMap", [(x) => [x, -x]], fresh.flatMap)),
  String(family([3, 1, 2], "parallelSort", [(x, y) => y - x], fresh.sort)),
  String(family(new Float64Array(big), "parallelMap", [square], fresh.tmap).slice(0, 5)),
  String(family(new Float64Array([3, 1, 2]), "parallelFilter", [(x) => x > 1], fresh.tfilter)),
  String(family(new Float64Array([3, 1, 2]), "parallelSort", [], fresh.tsort)),
];
Function.prototype.call = saved.call;
Array.prototype.map = saved.map; Array.prototype.filter = saved.filter;
Array.prototype.flatMap = saved.flatMap; Array.prototype.sort = saved.sort;
TypedProto.map = saved.tmap; TypedProto.filter = saved.tfilter; TypedProto.sort = saved.tsort;
console.log("prototype replaced", results.join(" | "));
console.log("unchanged", before.map === results[0], before.tmap === results[4]);
if (P) {
  const reduced = big.parallelReduce((acc, x) => acc + x, (l, r) => l + r, 0);
  console.log("reduce", reduced === big.reduce((acc, x) => acc + x, 0));
} else {
  console.log("reduce", true);
}
