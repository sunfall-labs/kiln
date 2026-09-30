// `Object.defineProperties` silently dropped symbol-keyed descriptors: the key walk wrapped
// every key as a string-domain lookup, which never matches a symbol slot, so the descriptor
// was skipped without error. Fastify's `Reply.prototype[Symbol(fastify.context)]` getter died
// on it (`reply[kRouteContext]` undefined into `validate`/`onErrorHook`). Recorded from node.
const s = Symbol("s");
const o = {};
Object.defineProperties(o, {
  [s]: { value: 42, enumerable: true },
  [Symbol.iterator]: { value: "iter", enumerable: false },
  plain: { value: "p", enumerable: true },
});
console.log(o[s], o.plain, o[Symbol.iterator]);
console.log(Object.getOwnPropertySymbols(o).map(String).join(","));

// Accessor form: the getter must install and run with the receiver.
function R() { this.request = { ctx: "here" }; }
const ctx = Symbol("ctx");
Object.defineProperties(R.prototype, {
  [ctx]: { get() { return this.request.ctx; } },
});
console.log(new R()[ctx]);

// Proxy: the `defineProperty` trap names the key as the language does, and the
// no-trap forward defines on the target under the symbol rather than `"@@symbol:N"`.
const seen = [];
const proxy = new Proxy({}, {
  defineProperty(t, key, desc) {
    seen.push(typeof key === "symbol" ? key.toString() : key);
    return Reflect.defineProperty(t, key, desc);
  },
});
Object.defineProperties(proxy, { [s]: { value: 1, enumerable: true }, q: { value: 2, enumerable: true } });
console.log(seen.join(","), proxy[s], proxy.q);
const bare = new Proxy({}, {});
Object.defineProperties(bare, { [s]: { value: 3, enumerable: true } });
console.log(bare[s], Object.getOwnPropertySymbols(bare).map(String).join(","));
