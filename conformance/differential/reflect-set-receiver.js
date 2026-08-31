// `Reflect.set(target, key, value, receiver)` writes to the *receiver*, and what it is allowed
// to change there is decided by what the receiver already holds.
//
// The descriptor is the thing under test, not the value. Completing the partial descriptor
// `OrdinarySetWithOwnDescriptor` hands to `[[DefineOwnProperty]]` -- filling `writable`,
// `enumerable` and `configurable` in with their `false` defaults -- leaves the right value
// behind a frozen property, so the *second* write is refused and, in strict code, throws. That
// is one accepted write per property, which is what Vue-style reactivity looks like when it
// stops working.

// A receiver that already holds the property keeps its flags.
const target = { count: 0 };
const proxy = new Proxy(target, {
  set(t, k, v, r) { return Reflect.set(t, k, v, r); },
});
proxy.count = 1;
proxy.count = 2;
proxy.count = 3;
console.log("through-proxy", target.count, JSON.stringify(Object.getOwnPropertyDescriptor(target, "count")));

// A receiver that does not hold it gets `CreateDataProperty`: all three flags true, and the
// target is left alone.
const source = { x: 1 };
const fresh = {};
console.log("fresh", Reflect.set(source, "x", 5, fresh), source.x, JSON.stringify(Object.getOwnPropertyDescriptor(fresh, "x")));

// The same, reached through the prototype chain by an ordinary assignment.
const derived = Object.create({ y: 1 });
derived.y = 2;
console.log("shadowed", JSON.stringify(Object.getOwnPropertyDescriptor(derived, "y")));

// A non-writable own property on the receiver refuses the write, whatever the target says.
const readOnly = {};
Object.defineProperty(readOnly, "w", { value: 1, writable: false, enumerable: true, configurable: true });
console.log("read-only receiver", Reflect.set({ w: 0 }, "w", 2, readOnly), readOnly.w);

// An accessor on the receiver refuses it too, even one with a setter: the target's data
// property is what decided this was a data write.
let seen = "unset";
const accessor = { set a(v) { seen = v; } };
console.log("accessor receiver", Reflect.set({ a: 0 }, "a", 9, accessor), seen);

// A non-object receiver can hold nothing, so the write is refused rather than silently dropped.
console.log("primitive receiver", Reflect.set({ p: 0 }, "p", 1, 7));

// The receiver's own `defineProperty` trap sees the partial descriptor, not a completed one.
const seenKeys = [];
const trapped = new Proxy({ n: 0 }, {
  defineProperty(t, k, d) { seenKeys.push(Object.keys(d).join(",")); return Reflect.defineProperty(t, k, d); },
});
Reflect.set({ n: 0 }, "n", 4, trapped);
console.log("trap descriptor", seenKeys.join("|"));
