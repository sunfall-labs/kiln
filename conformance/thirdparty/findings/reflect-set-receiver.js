// Minimal form of the vue-reactivity failure.
//
// `Reflect.set(target, key, value, receiver)` where the receiver already has an own data
// property must, per OrdinarySetWithOwnDescriptor, call `receiver.[[DefineOwnProperty]]` with a
// *partial* descriptor holding only [[Value]], leaving writable/enumerable/configurable alone.
// Kiln creates a fresh data property instead, so the first write through the proxy turns the
// property non-writable, non-enumerable and non-configurable. Every later write then returns
// false and is silently dropped in sloppy mode.
//
// node:  set count 1 -> true / 1 -> true / 2 -> true / 3 -> true; final 3 3
//        descriptor {"value":3,"writable":true,"enumerable":true,"configurable":true}
// kiln:  set count 1 -> true / 1 -> false / 2 -> false / 3 -> false; final 1 1
//        descriptor {"value":1,"writable":false,"enumerable":false,"configurable":false}
const target = { count: 0 };
const p = new Proxy(target, {
  get(t, k, r) { return Reflect.get(t, k, r); },
  set(t, k, v, r) {
    const res = Reflect.set(t, k, v, r);
    console.log("set", String(k), v, "->", res, "target now", t[k]);
    return res;
  },
});
p.count = 1;
p.count = 1;
p.count = 2;
p.count = 3;
console.log("final", target.count, p.count);
console.log("descriptor", JSON.stringify(Object.getOwnPropertyDescriptor(target, "count")));
