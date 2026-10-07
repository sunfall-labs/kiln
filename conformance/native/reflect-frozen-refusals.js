// What a non-extensible, sealed or frozen ordinary object answers through Reflect and Object.
// `Reflect.*` reports a refusal as `false` where `Object.*` throws a TypeError; a definition that
// changes nothing succeeds; deleting a name the object never had succeeds; and the prototype of a
// non-extensible object stays what it was, except for asking for the one it already has.
function attempt(label, f) {
  try { console.log(label, String(f())); } catch (e) { console.log(label, "threw", e.constructor.name); }
}
const make = {
  frozen: () => Object.freeze({ a: 1 }),
  sealed: () => Object.seal({ a: 1 }),
  closed: () => Object.preventExtensions({ a: 1 }),
  frozenArray: () => Object.freeze([1, 2]),
  frozenFn: () => Object.freeze(function f() {}),
};
for (const [kind, build] of Object.entries(make)) {
  const o = build();
  const proto = Object.getPrototypeOf(o);
  attempt(`${kind} Reflect.defineProperty new`, () => Reflect.defineProperty(o, "z", { value: 1 }));
  attempt(`${kind} Reflect.defineProperty same`, () => Reflect.defineProperty(o, "0" in o ? "0" : "a", { value: o["0" in o ? "0" : "a"] }));
  attempt(`${kind} Reflect.defineProperty change`, () => Reflect.defineProperty(o, "0" in o ? "0" : "a", { value: 99 }));
  attempt(`${kind} Reflect.defineProperty accessor`, () => Reflect.defineProperty(o, "0" in o ? "0" : "a", { get() { return 1; } }));
  attempt(`${kind} Object.defineProperty new`, () => Object.defineProperty(o, "z", { value: 1 }) === o);
  attempt(`${kind} Reflect.setPrototypeOf null`, () => Reflect.setPrototypeOf(o, null));
  attempt(`${kind} Reflect.setPrototypeOf same`, () => Reflect.setPrototypeOf(o, proto));
  attempt(`${kind} Reflect.setPrototypeOf other`, () => Reflect.setPrototypeOf(o, {}));
  attempt(`${kind} Object.setPrototypeOf null`, () => Object.setPrototypeOf(o, null) === o);
  attempt(`${kind} Object.setPrototypeOf same`, () => Object.setPrototypeOf(o, proto) === o);
  attempt(`${kind} proto kept`, () => Object.getPrototypeOf(o) === proto);
  attempt(`${kind} Reflect.deleteProperty missing`, () => Reflect.deleteProperty(o, "missing"));
  attempt(`${kind} Reflect.deleteProperty present`, () => Reflect.deleteProperty(o, "0" in o ? "0" : "a"));
  attempt(`${kind} Reflect.preventExtensions`, () => Reflect.preventExtensions(o));
  attempt(`${kind} Reflect.isExtensible`, () => Reflect.isExtensible(o));
  attempt(`${kind} Reflect.set present`, () => Reflect.set(o, "0" in o ? "0" : "a", 5));
  attempt(`${kind} Reflect.set new`, () => Reflect.set(o, "z", 5));
  attempt(`${kind} Reflect.has`, () => Reflect.has(o, "0" in o ? "0" : "a"));
  attempt(`${kind} keys`, () => Reflect.ownKeys(o).filter((k) => k !== "prototype").join());
}
