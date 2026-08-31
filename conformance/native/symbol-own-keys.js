// `Reflect.ownKeys` reports symbol-keyed properties, and reports no internal slot.
//
// A symbol key is a reserved *string* in both engines -- `@@iterator`, `@@symbol:3` -- and so is
// every internal slot the machine keeps beside it. Neither engine could tell them apart, so
// both reported neither, and every faithful-copy routine silently dropped symbol-keyed data.
// Both now keep a registry of the symbols they have made, which is the only thing that can turn
// a key back into the symbol it names: the same symbol, by identity, not a fresh one carrying
// the same description.
//
// The internal slots are the other half of the assertion, and the reason the registry is needed
// rather than a prefix test. A `Map` and an `Error` both carry reserved keys that are not
// properties of the object, so an engine that started reporting symbols by reporting every `@@`
// key would have made `Reflect.ownKeys(new Map())` list the collection's own storage.
//
// What is NOT asserted here, because the two engines do not yet agree: the native backend marks
// a symbol-keyed property non-enumerable, so `Object.assign` and object spread drop them there
// and `getOwnPropertyDescriptor(o, sym).enumerable` answers false. The rule is in
// `kiln_c99_own_slot`, which makes every `@@` key non-enumerable so that the string walks skip
// the internal ones; correcting it means separating the two questions at eight enumeration
// sites. conformance/differential/symbol-reflection.js asserts the whole surface against node.
const a = Symbol("a");
const b = Symbol("b");
const target = { x: 1, [a]: 2, 0: "zero", [b]: 3, y: 4 };
console.log("ownKeys", Reflect.ownKeys(target).map(String).join("|"));
console.log("identity", Reflect.ownKeys(target)[3] === a, Reflect.ownKeys(target)[4] === b);
console.log("names", Object.getOwnPropertyNames(target).join(","), "keys", Object.keys(target).join(","));
console.log("json", JSON.stringify(target));

// A copy that walks the keys keeps them, which is the whole point.
const clone = {};
for (const key of Reflect.ownKeys(target)) clone[key] = target[key];
console.log("clone", clone[a], clone[b], Reflect.ownKeys(clone).map(String).join("|"));
console.log("descriptors", Reflect.ownKeys(Object.getOwnPropertyDescriptors(target)).map(String).join("|"));
console.log("well-known", Reflect.ownKeys({ [Symbol.iterator]: 1, [Symbol.toStringTag]: 2, n: 3 }).map(String).join("|"));

// The machine's own reserved keys stay invisible.
console.log("internals", Reflect.ownKeys(new Map([[1, 2]])).length, Reflect.ownKeys(new Error("boom")).map(String).join("|"));
console.log("proxy", Reflect.ownKeys(new Proxy({ [a]: 1, z: 2 }, {})).map(String).join("|"));
console.log("empty", Reflect.ownKeys({}).length);
