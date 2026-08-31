// Symbol-keyed properties exist and read back, but no reflection API reports them.
//
// `Object.getOwnPropertySymbols` always answers `[]` and `Reflect.ownKeys` omits symbol keys, so
// every faithful-copy routine -- deep clone, deep merge, a decorator that re-wraps an object,
// `Object.assign` over `Reflect.ownKeys` -- silently drops symbol-keyed data instead of failing.
//
// node:  literal-read 1 symbols 1 ownKeys 2 ... wellknown 1 ... proto-symbols 1
// kiln:  literal-read 1 symbols 0 ownKeys 1 ... wellknown 0 ... proto-symbols 0
const k = Symbol("k");
const lit = { [k]: 1, a: 2 };
console.log("literal-read", lit[k], "symbols", Object.getOwnPropertySymbols(lit).length, "ownKeys", Reflect.ownKeys(lit).length);
const assigned = {}; assigned[k] = 1; assigned.a = 2;
console.log("assigned-read", assigned[k], "symbols", Object.getOwnPropertySymbols(assigned).length, "ownKeys", Reflect.ownKeys(assigned).length);
const defined = {}; Object.defineProperty(defined, k, { value: 3, enumerable: true, configurable: true, writable: true });
console.log("defined-read", defined[k], "symbols", Object.getOwnPropertySymbols(defined).length);
console.log("wellknown", Object.getOwnPropertySymbols({ [Symbol.iterator]: 1 }).length);
class C { [k]() { return "m"; } }
console.log("class-method", typeof new C()[k], "proto-symbols", Object.getOwnPropertySymbols(C.prototype).length);
