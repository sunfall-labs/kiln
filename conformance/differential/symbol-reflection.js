// Symbol-keyed properties are reported by reflection, not only stored.
//
// A symbol key is a reserved *string* here -- `@@iterator`, `@@symbol:3` -- and so is every
// internal slot the machine keeps beside it. Reflection could not tell the two apart, so it
// reported neither: `Object.getOwnPropertySymbols` always answered `[]` and `Reflect.ownKeys`
// omitted symbols, even though the properties were there and read back correctly. Every
// faithful-copy routine -- deep clone, deep merge, a decorator re-wrapping an object,
// `Object.assign` over `Reflect.ownKeys` -- silently dropped symbol-keyed data.
//
// The internal slots are the other half of the assertion. A `Map`, an `Error` and a
// `DisposableStack` all keep reserved keys, and none of them is a property of the object: an
// engine that started reporting symbols by reporting every `@@` key would have made
// `Reflect.ownKeys(new Map())` list the collection's own storage.
const a = Symbol("a");
const b = Symbol("b");
const target = { x: 1, [a]: 2, 0: "zero", [b]: 3, y: 4 };
console.log("ownKeys", Reflect.ownKeys(target).map(String).join("|"));
const found = Object.getOwnPropertySymbols(target);
console.log("identity", found.length, found[0] === a, found[1] === b, found[0].description);
console.log("names", Object.getOwnPropertyNames(target).join(","), "keys", Object.keys(target).join(","));
console.log("json", JSON.stringify(target));

// A copy that walks the keys keeps them, which is the whole point.
const clone = {};
for (const key of Reflect.ownKeys(target)) clone[key] = target[key];
console.log("clone", clone[a], clone[b], Object.getOwnPropertySymbols(clone).length);
console.log("assign", Object.getOwnPropertySymbols(Object.assign({}, target)).length);
console.log("descriptors", Object.getOwnPropertySymbols(Object.getOwnPropertyDescriptors(target)).length);

const anonymous = Symbol();
console.log("no-description", Object.getOwnPropertySymbols({ [anonymous]: 1 })[0] === anonymous, String(anonymous));
const shared = Symbol.for("shared");
console.log("registered", Object.getOwnPropertySymbols({ [shared]: 1 })[0] === Symbol.for("shared"), Symbol.keyFor(shared));
console.log("well-known", Object.getOwnPropertySymbols({ [Symbol.iterator]: 1, [Symbol.toStringTag]: 2 }).map(String).join("|"));

class C { [a]() {} static [b]() {} }
const array = [1];
array[a] = 2;
function fn() {}
fn[a] = 1;
console.log("carriers", Object.getOwnPropertySymbols(C.prototype).length, Object.getOwnPropertySymbols(C).length, Object.getOwnPropertySymbols(array).length, Object.getOwnPropertySymbols(fn).length);
console.log("array-keys", Reflect.ownKeys(array).map(String).join("|"));

// The machine's own reserved keys stay invisible.
console.log("internals", Object.getOwnPropertySymbols(new Map([[1, 2]])).length, Reflect.ownKeys(new Map([[1, 2]])).length, Reflect.ownKeys(new Error("boom")).map(String).join("|"));
console.log("empty", Object.getOwnPropertySymbols({}).length, Object.getOwnPropertySymbols([]).length);
