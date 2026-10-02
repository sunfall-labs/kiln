// ADR 0073: a shared value is a frozen graph, and every Reflect and Object operation answers on
// it what it answers on a frozen ordinary object. Each line runs the operation on both and prints
// the two answers, so a shared value that diverges from its frozen twin shows on the line itself.
import { share } from "kiln:shared";

function answer(f) {
  try { return String(f()); } catch (e) { return "threw " + e.constructor.name; }
}
const deepFreeze = (o) => {
  for (const v of Object.values(o)) if (v && typeof v === "object") deepFreeze(v);
  return Object.freeze(o);
};
const source = () => ({ a: 1, nested: { b: 2 }, list: [1, 2] });
const s = share(source());
const f = deepFreeze(source());
const checks = [
  ["Reflect.preventExtensions", (o) => Reflect.preventExtensions(o)],
  ["Reflect.preventExtensions nested", (o) => Reflect.preventExtensions(o.nested)],
  ["Reflect.preventExtensions list", (o) => Reflect.preventExtensions(o.list)],
  ["Object.preventExtensions", (o) => Object.preventExtensions(o) === o],
  ["Reflect.isExtensible", (o) => Reflect.isExtensible(o)],
  ["Object.isFrozen", (o) => Object.isFrozen(o)],
  ["Object.isSealed", (o) => Object.isSealed(o)],
  ["Reflect.defineProperty new", (o) => Reflect.defineProperty(o, "z", { value: 1 })],
  ["Reflect.defineProperty same", (o) => Reflect.defineProperty(o, "a", { value: 1 })],
  ["Reflect.defineProperty change", (o) => Reflect.defineProperty(o, "a", { value: 2 })],
  ["Reflect.defineProperty writable", (o) => Reflect.defineProperty(o, "a", { writable: true })],
  ["Object.defineProperty change", (o) => Object.defineProperty(o, "a", { value: 2 }) === o],
  ["Reflect.setPrototypeOf null", (o) => Reflect.setPrototypeOf(o, null)],
  ["Reflect.setPrototypeOf same", (o) => Reflect.setPrototypeOf(o, Object.getPrototypeOf(o))],
  ["Object.setPrototypeOf null", (o) => Object.setPrototypeOf(o, null) === o],
  ["Reflect.deleteProperty present", (o) => Reflect.deleteProperty(o, "a")],
  ["Reflect.deleteProperty missing", (o) => Reflect.deleteProperty(o, "missing")],
  ["Reflect.set present", (o) => Reflect.set(o, "a", 5)],
  ["Reflect.set new", (o) => Reflect.set(o, "z", 5)],
  ["Reflect.get", (o) => Reflect.get(o, "a")],
  ["Reflect.has own", (o) => Reflect.has(o, "a")],
  ["Reflect.has inherited", (o) => Reflect.has(o, "toString")],
  ["Reflect.has missing", (o) => Reflect.has(o, "missing")],
  ["in own", (o) => "a" in o],
  ["in inherited", (o) => "hasOwnProperty" in o],
  ["in list index", (o) => 1 in o.list],
  ["in list length", (o) => "length" in o.list],
  ["in list method", (o) => "map" in o.list],
  ["Reflect.has list", (o) => Reflect.has(o.list, 0)],
  ["Object.hasOwn", (o) => Object.hasOwn(o, "a")],
  ["ownKeys", (o) => Reflect.ownKeys(o).join()],
  ["descriptor", (o) => JSON.stringify(Reflect.getOwnPropertyDescriptor(o, "a"))],
  ["Object.freeze", (o) => Object.freeze(o) === o],
  ["Object.seal", (o) => Object.seal(o) === o],
];
for (const [label, check] of checks) {
  const shared = answer(() => check(s));
  const frozen = answer(() => check(f));
  console.log(label, shared === frozen ? "same" : "DIFFERS", shared, frozen);
}
console.log("value kept", s.a, Object.getPrototypeOf(s) === Object.prototype);
