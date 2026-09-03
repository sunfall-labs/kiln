// What a reactive proxy library does to an array and to a Map, reduced from `vue-reactivity`.
//
// The bundle diverged twice -- it ran to completion and printed different bytes, which is worse
// than a refusal -- and both causes were the native backend answering a question it should not
// have answered at all.
//
// FIRST: `Reflect.set` probes whether the write would be accepted before performing it, and read
// that answer out of the property table. An array's elements and its `length` are not in that
// table, and an array nobody has named a property on has no table, so the probe said "no" for
// every index. `Reflect.set(a, "3", 4, a)` reported false and dropped the element. Vue forwards
// every write through `Reflect.set` inside its set trap, and its `push` is
// `Array.prototype.push.apply(proxy, items)`, so the write vanished and the length dependency
// never fired: the effect ran once and never again.
//
// SECOND: `Object.isExtensible` answered false for a Map, because a collection carries no
// extensibility slot and "no slot" was read as "not extensible". Nothing had prevented anything;
// the language says true. Vue's `getTargetType` asks before it will wrap a collection and treats
// false as "do not wrap", so a reactive Map was handed back raw. And `"get" in map` answered
// false about a method the map demonstrably has, which is the other half of the same decision:
// vue installs its tracking instrumentations only for a key that is `in` the raw target.
//
// The rule both halves share is the one `absent-names-and-extensibility.ts` states for names: a
// question the backend cannot record the answer to is a refusal, but a question it knows the
// answer to must not be answered wrongly because the storage happens to live somewhere else.

// --- Reflect.set reaches an array's elements -------------------------------------------------

const forward = <T extends object>(target: T): T =>
  new Proxy(target, { set: (o, k, v, r) => Reflect.set(o, k, v, r) });

const existing = [1, 2, 3];
forward(existing)[0] = 9;
console.log("existing", existing.join(","));

const appended = [1, 2, 3];
forward(appended)[3] = 4;
console.log("appended", appended.join(","), appended.length);

const truncated = [1, 2, 3];
(forward(truncated) as unknown as { length: number }).length = 1;
console.log("truncated", truncated.join(","), truncated.length);

const named = [1, 2, 3] as unknown as Record<string, unknown>;
forward(named).total = 7;
console.log("named", named.total, "total" in named, "nope" in named);

const plain = [1, 2, 3];
console.log("same receiver", Reflect.set(plain, "3", 4, plain), plain.join(","));

// `push` applied to the proxy is exactly what vue's array instrumentation does: the raw method,
// the proxy as `this`, so the element write and the `length` write go through the trap. Only the
// *result* is checked, not the trap sequence: the two engines disagree about how many element
// writes `push` performs on a proxy receiver, which is a separate defect recorded under
// `conformance/thirdparty/findings/`.
const pushed = [1, 2, 3];
const watcher = new Proxy(pushed, { set: (o, k, v, r) => Reflect.set(o, k, v, r) });
console.log("push result", Array.prototype.push.apply(watcher, [4]));
console.log("push landed", pushed.join(","), pushed.length);

// --- A collection answers `in`, absent names, and extensibility ------------------------------

const map = new Map<string, number>([["a", 1]]);
const set = new Set<number>([1, 2]);
const weak = new WeakMap<object, number>();

for (const name of ["get", "set", "add", "has", "delete", "clear", "size", "keys", "values",
                    "entries", "forEach", "constructor", "hasOwnProperty", "nope", "a"]) {
  console.log("in map", name, name in map, "| in set", name in set);
}
// The weak pair separately, and without `clear`: the interpreter reports one on a `WeakMap`
// where node does not, so that name is the interpreter's defect rather than this corpus's
// question. Everything else here the two engines and node agree on.
for (const name of ["get", "set", "add", "has", "delete", "size", "keys", "values", "entries",
                    "forEach", "constructor", "hasOwnProperty", "nope"]) {
  console.log("in weak", name, name in weak);
}
console.log("iterator", Symbol.iterator in map, Symbol.iterator in set, Symbol.iterator in weak);

// An absent name on a collection is `undefined`, exactly as it is on an array or a date. A
// reactive proxy reads its own bookkeeping names off whatever it was handed.
for (const name of ["__v_isRef", "__v_skip", "nope", "then", "length"]) {
  console.log("absent", name, typeof (map as unknown as Record<string, unknown>)[name]);
}
// Nothing has prevented extensions on any of these, so every one of them is extensible.
console.log("extensible", Object.isExtensible(map), Object.isExtensible(set),
            Object.isExtensible(weak), Object.isExtensible(new Date(0)),
            Object.isExtensible(/a/), Object.isExtensible(Promise.resolve(1)));
console.log("primitives", Object.isExtensible(1 as never), Object.isExtensible("a" as never));
const prevented: Record<string, unknown> = { kept: 1 };
Object.preventExtensions(prevented);
console.log("still recorded", Object.isExtensible(prevented), Object.isExtensible({}),
            Object.isExtensible([]), Object.isExtensible(Object.freeze({ a: 1 })));

// The members themselves are untouched by the decision above.
console.log("members", typeof map.get, typeof map.set, typeof map.forEach, map.size,
            typeof set.add, set.size, typeof weak.get);
console.log("spread", JSON.stringify([...map]), JSON.stringify([...set]));
