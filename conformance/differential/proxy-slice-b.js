// Proxy-integrity methods must observe internal traps, and keep their target invariants.
const integrityLog = [];
const integrityTarget = { value: 1 };
const integrity = new Proxy(integrityTarget, {
  preventExtensions(t) { integrityLog.push("prevent"); return Reflect.preventExtensions(t); },
  ownKeys(t) { integrityLog.push("keys"); return Reflect.ownKeys(t); },
  getOwnPropertyDescriptor(t, k) {
    integrityLog.push("desc:" + k);
    return Reflect.getOwnPropertyDescriptor(t, k);
  },
  defineProperty(t, k, d) {
    integrityLog.push("define:" + k + ":" + d.configurable + ":" + d.writable);
    return Reflect.defineProperty(t, k, d);
  },
});
console.log(Object.freeze(integrity) === integrity, Object.isFrozen(integrity), integrityLog.join(","));

// Object.assign must use a proxy source's ownKeys/getOwnPropertyDescriptor/get traps and a
// proxy target's set trap.
const assignLog = [];
const assigned = new Proxy({}, {
  set(t, k, v, r) { assignLog.push("set:" + k + ":" + (r === assigned)); return Reflect.set(t, k, v, r); },
});
const source = new Proxy({ a: 2, b: 3 }, {
  ownKeys(t) { assignLog.push("keys"); return ["b", "a"]; },
  getOwnPropertyDescriptor(t, k) { assignLog.push("desc:" + k); return Reflect.getOwnPropertyDescriptor(t, k); },
  get(t, k, r) { assignLog.push("get:" + k + ":" + (r === source)); return Reflect.get(t, k, r); },
});
Object.assign(assigned, source);
console.log(assigned.a, assigned.b, assignLog.join(","));

// Reflect.set's optional receiver must reach a proxy set trap, including fallback through a
// proxy target with no set trap.
const setLog = [];
const base = { x: 1 };
const setProxy = new Proxy(base, {
  set(t, k, v, r) { setLog.push("trap:" + k + ":" + (r === receiver)); return Reflect.set(t, k, v, r); },
});
const receiver = {};
console.log(Reflect.set(setProxy, "x", 4, receiver), receiver.x, base.x, setLog.join(","));
const fallback = new Proxy({ y: 1 }, {});
const fallbackReceiver = {};
console.log(Reflect.set(fallback, "y", 5, fallbackReceiver), fallbackReceiver.y, fallback.y);

// All visible prototype walks must use a proxy's getPrototypeOf trap.
const proto = { marker: 1 };
const value = Object.create(proto);
const protoProxy = new Proxy(value, { getPrototypeOf() { return proto; } });
let listed = [];
for (const key in protoProxy) listed.push(key);
console.log(Object.getPrototypeOf(protoProxy) === proto, proto.isPrototypeOf(protoProxy), protoProxy instanceof Object, listed.join(","));

const sealedTarget = { x: 1 };
const sealedLog = [];
const sealed = new Proxy(sealedTarget, {
  preventExtensions(t) { sealedLog.push("prevent"); return Reflect.preventExtensions(t); },
  ownKeys(t) { sealedLog.push("keys"); return Reflect.ownKeys(t); },
  getOwnPropertyDescriptor(t, k) { sealedLog.push("desc:" + k); return Reflect.getOwnPropertyDescriptor(t, k); },
  defineProperty(t, k, d) { sealedLog.push("define:" + k); return Reflect.defineProperty(t, k, d); },
});
console.log(Object.seal(sealed) === sealed, Object.isSealed(sealed), sealedLog.join(","));

const altered = new Proxy({}, { getPrototypeOf() { return null; } });
console.log(altered instanceof Object, Object.prototype.isPrototypeOf(altered));

const accessorTarget = Object.create({ set x(v) { this.seen = v; } });
const accessorProxy = new Proxy(accessorTarget, {});
const accessorReceiver = {};
console.log(Reflect.set(accessorProxy, "x", 7, accessorReceiver), accessorReceiver.seen, accessorTarget.seen);

const receiverProxyLog = [];
const receiverProxy = new Proxy({}, {
  defineProperty(t, k, d) { receiverProxyLog.push("define:" + k); return Reflect.defineProperty(t, k, d); },
});
console.log(Reflect.set({ x: 1 }, "x", 8, receiverProxy), receiverProxy.x, receiverProxyLog.join(","));

const frozenArray = Object.freeze(new Proxy([1, 2], {}));
console.log(Object.isFrozen(frozenArray), Object.isSealed(frozenArray));
