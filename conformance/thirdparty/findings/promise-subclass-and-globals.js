// The parts of the Promise cluster real code reaches.
//
// A Promise subclass inherits no statics, so `MyPromise.resolve` and `MyPromise.all` are
// undefined -- the shape every cancellable-promise and bluebird-interop layer uses. `Promise.any`
// still rejects with something carrying `.errors`, but it is not an `AggregateError`, so
// `e.constructor.name` answers "Error": a plausible wrong answer rather than a refusal.
//
// node:  subclass function|function|true  aggregate AggregateError|AggregateError|1|msg|true
//        globals function|function|function|function
// kiln:  subclass undefined|undefined|true
//        aggregate THREW ReferenceError: AggregateError is not defined
//        globals undefined|undefined|function|undefined
class P extends Promise {}
console.log("subclass", typeof P.resolve, typeof P.all, P[Symbol.species] === P);
try {
  const e = new AggregateError([new Error("a")], "msg");
  console.log("aggregate", e.constructor.name, e.name, e.errors.length, e.message, e instanceof Error);
} catch (e) { console.log("aggregate THREW", e.constructor.name + ":", e.message); }
console.log("globals", typeof AbortController, typeof AbortSignal, typeof queueMicrotask, typeof structuredClone);
Promise.any([Promise.reject(new Error("x")), Promise.reject(new Error("y"))]).catch((e) => {
  console.log("any-rejection", e.constructor.name, e.errors ? e.errors.map((x) => x.message).join("") : "no-errors");
});
