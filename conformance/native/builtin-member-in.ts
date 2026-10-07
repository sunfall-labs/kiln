// `in` on a built-in prototype, and the own-name listing beside it. The native backend answered
// `false` for every member of every built-in prototype -- `"map" in Array.prototype` included --
// because `kiln_c99_has_property` had no arm for a built-in and fell through to the object chain
// walk, which stops at the first value that is not an object. A feature test then took the branch
// written for a realm that never had the member.
const protos: [string, object, string][] = [
  ["Array", Array.prototype, "map"],
  ["String", String.prototype, "charAt"],
  ["Object", Object.prototype, "hasOwnProperty"],
  ["Number", Number.prototype, "toFixed"],
  ["Function", Function.prototype, "call"],
  ["Promise", Promise.prototype, "then"],
  ["Map", Map.prototype, "get"],
  ["Set", Set.prototype, "add"],
  ["Date", Date.prototype, "getTime"],
  ["RegExp", RegExp.prototype, "test"],
];
for (const [name, proto, member] of protos) {
  console.log(name, member in proto, "hasOwnProperty" in proto, "nopeNopeNope" in proto);
}

console.log("inst", "map" in [], "nope" in [], "then" in Promise.resolve(1),
  "valueOf" in [], "hasOwnProperty" in []);
console.log("ns", "floor" in Math, "nope" in Math, "stringify" in JSON, "nope" in JSON);
console.log("ctor", "keys" in Object, "nope" in Object, "isArray" in Array, "nope" in Array);

// The four collection prototypes do not share a member list. Reading a sibling kind's member is
// `undefined` -- absent, not unbuilt -- and `in` says so too.
console.log("kinds", "get" in Map.prototype, "add" in Map.prototype, "add" in Set.prototype, "get" in Set.prototype);
console.log("weak", "get" in WeakMap.prototype, "add" in WeakMap.prototype, "add" in WeakSet.prototype, "get" in WeakSet.prototype);
console.log("kindread", typeof Map.prototype.add, typeof Set.prototype.get, typeof (WeakSet.prototype as any).get);

// Built-in constructors with a shared member surface still answer genuinely absent names as
// undefined. `kind-of` probes `value.constructor.isBuffer` for arbitrary values, including Set;
// Set does not have an `isBuffer` property and that probe must take its ordinary fallback.
console.log("missing-constructor-members", typeof (Set as any).isBuffer,
  (Set as any).isBuffer === undefined, "isBuffer" in (Set as any));

// A string carries `@@iterator` and no `values` beside it.
console.log("strvalues", typeof String.prototype.values, typeof "ab".values, [..."ab"].join("-"));

// A member the language has and this backend has not built is still *present*: `in` asks whether
// the property exists, and a read is the operation that refuses. The interpreter answers from
// what it owns, so this asks only about names both engines have built.
console.log("lang", "anchor" in String.prototype, "nopeNope" in String.prototype, "getUTCDate" in Date.prototype);

// A symbol key reaches this layer as `@@name` whether it is well-known or came from
// `Symbol.for`, and two membership predicates answer every one of them -- right for the reads
// they serve, wrong for `in`. A bundled `date-fns` guards on
// `Symbol.for("constructDateFrom") in date` and called the undefined it found when this said yes.
console.log("sym", Symbol.iterator in Array.prototype, Symbol.for("x") in Array.prototype,
  Symbol.toPrimitive in new Date(0), Symbol.for("constructDateFrom") in new Date(0));
