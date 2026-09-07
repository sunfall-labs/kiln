"use strict";

// Regression coverage for zero-backed biased dictionary indices.  The explicit descriptor and
// delete/redefine operations force the dictionary path instead of relying on object-literal shape
// construction.  Keep every assertion scalar so the interpreter/native transcript is stable.
const proto = {};
Object.defineProperty(proto, "inherited", { value: "proto", writable: true, enumerable: true, configurable: true });
let getterReads = 0;
let setterValue = "unset";
Object.defineProperty(proto, "accessor", {
  get() { getterReads += 1; return "get-" + getterReads; },
  set(value) { setterValue = value; },
  enumerable: true,
  configurable: true,
});

const object = Object.create(proto);
Object.defineProperty(object, "first", { value: 1, writable: true, enumerable: true, configurable: true });
Object.defineProperty(object, "second", { value: 2, writable: true, enumerable: true, configurable: true });
const secondObject = Object.create(proto);
Object.defineProperty(secondObject, "second", { value: 4, writable: true, enumerable: true, configurable: true });
Object.defineProperty(secondObject, "first", { value: 3, writable: true, enumerable: true, configurable: true });

// These helpers deliberately reuse the same read/write/getter sites across two dictionary
// objects.  The second object also takes the prototype-getter path before acquiring an own
// shadow, so a cached index cannot be mistaken for a property identity.
function readWritePair(target, value) {
  target.first = value;
  return target.first + target.second;
}
function readPrototypeValue(target) { return target.accessor; }
console.log("reused", readWritePair(object, 11), readWritePair(secondObject, 13), readWritePair(object, 15));
console.log("shared getter", readPrototypeValue(object), readPrototypeValue(secondObject), getterReads);
Object.defineProperty(secondObject, "accessor", { value: "own-accessor", writable: true, enumerable: true, configurable: true });
console.log("getter shadow", readPrototypeValue(secondObject), readPrototypeValue(object), getterReads);
delete secondObject.accessor;
console.log("getter after delete", readPrototypeValue(secondObject), getterReads);
console.log("own", object.first, object.second, Object.hasOwn(object, "first"), Object.hasOwn(object, "missing"));
console.log("prototype", object.inherited, "inherited" in object, Object.hasOwn(object, "inherited"));

object.first = 10;
object.inherited = "own-now";
console.log("writes", object.first, object.inherited, proto.inherited, Object.hasOwn(object, "inherited"));
console.log("accessor", object.accessor, getterReads);
object.accessor = "set-value";
console.log("setter", setterValue, getterReads);

Object.defineProperty(object, "inherited", { value: "redefined", writable: true, enumerable: false, configurable: true });
console.log("shadow", object.inherited, Object.hasOwn(object, "inherited"), Object.keys(object).join(","));
delete object.second;
console.log("delete", object.second, Object.hasOwn(object, "second"));
Object.defineProperty(object, "second", { value: 22, writable: true, enumerable: true, configurable: true });
console.log("redefine", object.second);

const protoName = { value: "proto-name" };
Object.defineProperty(object, "__proto__", { value: protoName, writable: true, enumerable: true, configurable: true });
console.log("special", object.__proto__ === protoName, Object.hasOwn(object, "__proto__"));
Object.defineProperty(object, "normalHidden", { value: "hidden-value", writable: true, enumerable: false, configurable: true });
console.log("hidden", object.normalHidden, Object.keys(object).includes("normalHidden"), object["@@missing"]);
const symbolName = Symbol("dictionary");
Object.defineProperty(object, symbolName, { value: "symbol-value", writable: true, enumerable: true, configurable: true });
console.log("symbol", object[symbolName], Object.getOwnPropertySymbols(object).length > 0);

const frozen = Object.freeze(Object.create(null));
console.log("frozen", Object.isFrozen(frozen), Object.getPrototypeOf(frozen) === null);
try { Object.defineProperty(frozen, "newName", { value: 1 }); console.log("freeze-failed"); }
catch (error) { console.log("freeze", error instanceof TypeError); }
console.log("missing", object.notThere, "notThere" in object, Object.hasOwn(object, "notThere"));
