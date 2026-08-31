// Vue 3's reactivity core: dependency tracking built entirely out of Proxy + Reflect.
//
// A different proxy shape from immer's copy-on-write draft. Vue installs distinct handlers for
// object, array, collection (Map/Set) and readonly targets, dispatches through `Reflect.get` with
// an explicit receiver so getters see the proxy, keys identity with WeakMap, and patches the
// array search methods and the collection iterators by hand. Almost every proxy trap is reached.
import {
  reactive, readonly, shallowReactive, ref, shallowRef, computed, effect,
  toRaw, markRaw, isReactive, isRef, unref, effectScope, stop,
} from "@vue/reactivity";

const log = [];

// Plain object: get/set/has/deleteProperty/ownKeys.
const state = reactive({ count: 0, nested: { deep: 1 }, list: [1, 2, 3] });
effect(() => log.push("count=" + state.count));
state.count = 1;
state.count = 1; // no re-run: the setter compares old and new
state.count = 2;
console.log("obj", log.join("|"));
console.log("nested-is-reactive", isReactive(state.nested), "raw-is-not", isReactive(toRaw(state)));

// has / ownKeys / delete are separate traps with their own dep keys.
const keys = [];
effect(() => keys.push(Object.keys(state).join(",")));
state.extra = 9;
delete state.extra;
console.log("keys", keys.join(" / "));

const hits = [];
effect(() => hits.push("has:" + ("maybe" in state)));
state.maybe = 1;
console.log("has", hits.join("|"));

// Arrays: length tracking, and the patched includes/indexOf that must look through the proxy.
const arrLog = [];
effect(() => arrLog.push("len=" + state.list.length + " sum=" + state.list.reduce((a, b) => a + b, 0)));
state.list.push(4);
console.log("array", arrLog.join("|"));
const objs = reactive([{ id: 1 }]);
console.log("includes-raw", objs.includes(toRaw(objs)[0]), "indexOf-proxy", objs.indexOf(objs[0]));

// Collections: reactive Map/Set go through a wholly separate handler that returns bound methods.
const m = reactive(new Map([["a", 1]]));
const s = reactive(new Set([1, 2]));
const collLog = [];
effect(() => collLog.push("m=" + m.get("a") + " size=" + m.size + " s=" + [...s].join("+")));
m.set("a", 2);
s.add(3);
console.log("collection", collLog.join("|"));
console.log("map-entries", JSON.stringify([...m.entries()]), "set-values", JSON.stringify([...s.values()]));
let forEachSeen = "";
m.forEach((v, k) => { forEachSeen += k + ":" + v + ";"; });
console.log("map-forEach", forEachSeen, "map-is-reactive-value", isReactive(reactive(new Map([["k", {}]])).get("k")));

// refs: a class with an accessor pair, unwrapped automatically inside a reactive object.
const r = ref(1);
const c = computed(() => r.value * 10);
const compLog = [];
effect(() => compLog.push("c=" + c.value));
r.value = 2;
r.value = 3;
console.log("computed", compLog.join("|"), "isRef", isRef(r), unref(r), c.value);
const holder = reactive({ r });
console.log("ref-unwrap", holder.r, isRef(holder.r));

// readonly: a proxy whose set trap warns and returns true, and shallow variants.
const ro = readonly({ a: 1, b: { c: 2 } });
ro.a = 5;
console.log("readonly", ro.a, isReactive(ro.b) === false);
const sh = shallowReactive({ a: { b: 1 } });
console.log("shallow", isReactive(sh.a), isRef(shallowRef({})));

// markRaw is a non-enumerable brand read back through the proxy.
const skipped = reactive({ raw: markRaw({ n: 1 }) });
console.log("markRaw", isReactive(skipped.raw));

// effectScope: stopping a scope must detach every effect registered under it.
const scope = effectScope();
const scoped = [];
scope.run(() => { effect(() => scoped.push(state.count)); });
state.count = 3;
scope.stop();
state.count = 4;
console.log("scope", scoped.join(","));

// A stopped effect must not re-run, and `toRaw` of a raw value is itself.
const stoppedLog = [];
const runner = effect(() => stoppedLog.push(state.count), { lazy: false });
stop(runner);
state.count = 5;
console.log("stopped", stoppedLog.join(","), toRaw(42));
