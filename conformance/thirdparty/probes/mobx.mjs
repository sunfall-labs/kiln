// MobX: transparent reactive state.
//
// A third proxy shape, distinct from immer's drafts and Vue's dep tracking. MobX builds an
// observable *object graph* out of `Object.defineProperty` accessor pairs behind a Proxy,
// annotates classes through a decorator-free `makeObservable` that reads the prototype's own
// descriptors, batches in transactions, and derives computeds lazily with cycle detection. It is
// the input in this corpus that leans hardest on property descriptors and `Symbol` brands.
import {
  observable, computed, autorun, reaction, action, runInAction, makeAutoObservable,
  toJS, isObservable, isObservableProp, observe, when, configure, intercept,
} from "mobx";

configure({ enforceActions: "never", useProxies: "always" });

const lines = [];
const say = (...a) => lines.push(a.join(" "));

// Observable object: defineProperty-backed accessors under a proxy.
const store = observable({ first: "ada", last: "lovelace", tags: ["a"], meta: new Map([["k", 1]]) });
const full = computed(() => store.first + " " + store.last);
const seen = [];
const disposeAuto = autorun(() => seen.push(full.get()));
store.first = "grace";
runInAction(() => { store.first = "alan"; store.last = "turing"; });
say("autorun", seen.join("|"));
say("isObservable", isObservable(store), isObservableProp(store, "first"), isObservable(store.tags));

// Observable arrays: a proxied array whose length and index writes both notify.
const arrSeen = [];
const disposeArr = autorun(() => arrSeen.push(store.tags.length + ":" + store.tags.join("+")));
store.tags.push("b");
store.tags[0] = "z";
store.tags.splice(1, 1);
say("array", arrSeen.join("|"));
say("array-is-real", Array.isArray(store.tags), JSON.stringify(toJS(store.tags)));

// Observable maps: a wrapper class, not a Proxy, with its own has/get/keys deps.
const mapSeen = [];
const disposeMap = autorun(() => mapSeen.push(store.meta.get("k") + "/" + store.meta.size + "/" + (store.meta.has("new") ? "y" : "n")));
store.meta.set("k", 2);
store.meta.set("new", 3);
say("map", mapSeen.join("|"));
say("map-spread", JSON.stringify([...store.meta.entries()]));

// reaction: an explicit data function plus an effect, with a custom comparer.
const reacted = [];
const disposeReaction = reaction(() => ({ f: store.first }), (v) => reacted.push(v.f), { equals: (a, b) => a.f === b.f });
store.first = "alan";      // structurally equal: no fire
store.first = "edsger";
say("reaction", reacted.join(","));

// A class made observable, with an action and a computed getter on the prototype.
class Counter {
  n = 0;
  constructor() { makeAutoObservable(this); }
  get doubled() { return this.n * 2; }
  inc() { this.n++; }
}
const counter = new Counter();
const counted = [];
const disposeCount = autorun(() => counted.push(counter.doubled));
counter.inc();
counter.inc();
say("class", counted.join(","), isObservableProp(counter, "n"), typeof counter.inc);

// observe / intercept: the low-level change pipeline.
const changes = [];
const disposeObserve = observe(store, "last", (c) => changes.push(c.type + ":" + c.oldValue + ">" + c.newValue));
const disposeIntercept = intercept(store, "last", (c) => ({ ...c, newValue: String(c.newValue).toUpperCase() }));
store.last = "hopper";
say("observe", changes.join(","), store.last);

// `when` returns a promise that resolves once a predicate turns true.
const p = when(() => store.first === "target");
action(() => { store.first = "target"; })();
p.then(() => {
  say("when", "resolved", store.first);
  say("toJS", JSON.stringify(toJS({ f: store.first, t: store.tags })));
  disposeAuto(); disposeArr(); disposeMap(); disposeReaction(); disposeCount();
  disposeObserve(); disposeIntercept();
  console.log(lines.join("\n"));
});
