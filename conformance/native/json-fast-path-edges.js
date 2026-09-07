// Edge cases specific to the JSON fast paths added in this change (the shaped-object walker that
// skips `toJSON` and `own_keys`, the interned-key/shape-transition parser, and the Clinger
// fast-path number parser). `json-roundtrip-edge.js` already covers the general JSON surface;
// this file targets exactly the corners those fast paths could get wrong if the "common case"
// guess were not conservative enough, with node as the oracle for every line.

// `toJSON` on an ancestor that is not the object's own shape, added and then removed: the fast
// path's whole premise is that a plain object's chain has no `toJSON` anywhere on it, and this is
// the ordinary way that becomes false without putting `toJSON` on the object itself, so the slow
// path -- the real `kiln_c99_get` walk -- is what has to notice. (`Object.prototype` itself is
// deliberately not used for this: assigning a new property onto it is a pre-existing limitation
// of this backend unrelated to JSON -- confirmed with `({}).toJSON` throwing "not a function"
// right after the same assignment, with no JSON code anywhere in that call -- so patching it
// would test that gap instead of this change.)
const protoWithToJSON = { toJSON() { return "patched"; } };
const child = Object.create(protoWithToJSON);
child.a = 1;
console.log(JSON.stringify(child));
console.log(JSON.stringify({ x: child }));
console.log(JSON.stringify([child]));
delete protoWithToJSON.toJSON;
console.log(JSON.stringify(child));

// Integer-like keys sort before string keys, in ascending numeric order, ahead of every string
// key in the order it was inserted -- the ordinary property-enumeration rule, which the shaped
// fast path must still honour since it walks the dense `elements` store before `shape`.
console.log(JSON.stringify({ b: 1, 2: 1, a: 1, 1: 1 }));

// A getter member: still a value read by *calling*, in the dictionary-mode path this change also
// touched (a shape has no slot for an accessor, so defining one moves the object out of the
// shaped fast path entirely).
const withGetter = { a: 1, get b() { return 2; }, c: 3 };
console.log(JSON.stringify(withGetter));

// A non-enumerable member is invisible to `JSON.stringify` exactly as it is to `Object.keys`.
const withHidden = { a: 1 };
Object.defineProperty(withHidden, "hidden", { value: 99, enumerable: false });
console.log(JSON.stringify(withHidden));

// A symbol-keyed member never has a JSON spelling, and does not stop a plain key beside it from
// having one.
const withSymbol = { a: 1, [Symbol("s")]: 2, b: 3 };
console.log(JSON.stringify(withSymbol));

// `__proto__` as a key `JSON.parse` reads from text is data, not a request to relink the
// prototype -- unlike an object literal or a plain assignment, which is `kiln_c99_put`'s rule and
// exactly the rule this file's new `kiln_c99_json_define` must not inherit.
const parsedProto = JSON.parse('{"__proto__":{"x":1},"y":2}');
console.log(Object.getPrototypeOf(parsedProto) === Object.prototype);
console.log(Object.prototype.hasOwnProperty.call(parsedProto, "__proto__"));
console.log(parsedProto.__proto__.x, parsedProto.y);
console.log(JSON.stringify(parsedProto));

// A repeated key in a parsed document keeps the last value, which is exactly what overwriting an
// already-transitioned shape slot gives for free.
console.log(JSON.parse('{"a":1,"b":2,"a":3}').a);
console.log(JSON.stringify(JSON.parse('{"a":1,"b":2,"a":3}')));

// A document with enough distinct keys on one object to overflow the fast-property shape limit
// (KILN_SHAPE_MAX_FAST_PROPERTIES = 32): the parser's fast define must fall back to the ordinary
// dictionary store rather than lose or misplace a key past that point.
let manyKeys = "{";
for (let i = 0; i < 40; i++) manyKeys += (i ? "," : "") + '"k' + i + '":' + i;
manyKeys += "}";
const manyParsed = JSON.parse(manyKeys);
console.log(Object.keys(manyParsed).length, manyParsed.k0, manyParsed.k39);

// The number list the fast parser and the fast integer stringifier both have to agree with node
// on: values inside the Clinger fast-path range and values (deliberately) outside it, on both
// sides of the round trip.
const numbers = [
  1e21, 1e-7, 0.1, 123456789012345680000, 5e-324, 1.7976931348623157e308,
  9007199254740993, -0, 1e5, 1e-5, 0.30000000000000004, 0, -1, 42, -42,
  9007199254740991, -9007199254740991, 9007199254740992, 100, 1000000000000000,
];
for (const n of numbers) {
  const s = JSON.stringify(n);
  const roundtripped = JSON.parse(s);
  console.log(s, Object.is(roundtripped, n));
}
// The exact source spellings a fast-path parser could be tempted to mis-handle: explicit `+`
// exponent, uppercase `E`, and a value already covered above by its computed spelling.
console.log(JSON.parse("1e+5"), JSON.parse("1E5"), JSON.parse("-0"));
