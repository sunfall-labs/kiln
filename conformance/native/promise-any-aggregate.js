// `Promise.any` rejects with an `AggregateError`.
//
// Both engines built a plain `Error` carrying an `errors` array, which is a *plausible* wrong
// answer: everything a handler reads off it was right except what it is, so
// `e.constructor.name` answered "Error" and `e instanceof AggregateError` was unreachable.
//
// Here rather than in the differential corpus because it is the one part of `AggregateError`
// the native backend can reach: the global constructor iterates its first argument, and that
// runtime has no `IterableToList`, so it still refuses the name.
Promise.any([Promise.reject(new Error("x")), Promise.reject(new Error("y"))]).catch((e) => {
  console.log("any", e.constructor.name, e.name, e.message, e.errors.map((z) => z.message).join(""), String(e));
});
Promise.any([]).catch((e) => console.log("empty", e.constructor.name, e.errors.length, Object.keys(e).length));
