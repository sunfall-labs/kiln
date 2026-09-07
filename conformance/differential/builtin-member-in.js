// `in` on a built-in prototype: true for a member, false for a name no engine has. The
// interpreter answered true for *every* string here -- `"nope" in Object.prototype` among them --
// because it asked what a read finds rather than what the object owns, and a prototype table's
// read carries its own step up to `Object.prototype`. Feature detection is the caller that cares.
const protos = [
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
  // A member, a name inherited from `Object.prototype`, and one nothing has.
  console.log(name, member in proto, "hasOwnProperty" in proto, "nopeNopeNope" in proto);
}

// An instance answers the same way, and always did.
console.log("inst", "map" in [], "nope" in [], "charAt" in Object("x"), "then" in Promise.resolve(1));

// A namespace and a constructor are ordinary objects that inherit from `Object.prototype`.
console.log("ns", "floor" in Math, "nope" in Math, "stringify" in JSON, "nope" in JSON);
console.log("ctor", "keys" in Object, "nope" in Object, "isArray" in Array, "nope" in Array);

// The four collection prototypes do not share a member list: a Set has no `get`, a Map no `add`.
console.log("kinds", "get" in Map.prototype, "add" in Map.prototype, "add" in Set.prototype, "get" in Set.prototype);
console.log("weak", "get" in WeakMap.prototype, "add" in WeakMap.prototype, "add" in WeakSet.prototype, "get" in WeakSet.prototype);

// `in` agrees with `hasOwnProperty` about what a prototype owns, and neither invents a name.
const owns = (o, k) => Object.prototype.hasOwnProperty.call(o, k);
console.log("agree", owns(Array.prototype, "map"), "map" in Array.prototype, owns(Array.prototype, "nope"), "nope" in Array.prototype);

// A string carries `@@iterator` and no `values` beside it, unlike an array.
console.log("strvalues", typeof String.prototype.values, typeof "ab".values, [..."ab"].join("-"));

// A symbol key reaches this layer as `@@name` whether it is well-known or came from
// `Symbol.for`, and two membership predicates answer every one of them -- right for the reads
// they serve, wrong for `in`. A bundled `date-fns` guards on
// `Symbol.for("constructDateFrom") in date` and called the undefined it found when this said yes.
console.log("sym", Symbol.iterator in Array.prototype, Symbol.for("x") in Array.prototype,
  Symbol.toPrimitive in new Date(0), Symbol.for("constructDateFrom") in new Date(0));
