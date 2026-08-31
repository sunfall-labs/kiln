// `AggregateError`, the error kind that reports several failures as one.
//
// It did not exist, so `Promise.any`'s rejection was a plain `Error` carrying an `errors` array
// -- right in everything a handler reads off it except what it is -- and any code that wrote
// `new AggregateError(...)` itself got a `ReferenceError`.
//
// It is the same shape as `SuppressedError`: an `Error` with one carried field and a prototype
// chaining to `Error.prototype`. The difference is that its first argument is *iterated*
// (`IterableToList`, not an array-like read), so it is the one error constructor that can run a
// program's own code and fail.
const e = new AggregateError([new Error("a"), new Error("b")], "msg");
console.log("ctor", e.constructor.name, e.name, e.message, e.errors.length, e instanceof Error, e instanceof AggregateError);
console.log("own", JSON.stringify(Object.getOwnPropertyNames(e)));
console.log("string", String(e));
console.log("prototype", Object.getPrototypeOf(e) === AggregateError.prototype, AggregateError.prototype.name);

// Any iterable, not just an array, and the list it makes is a fresh array.
const iterated = new AggregateError(new Set([1, 2, 3]), "s");
console.log("iterable", iterated.errors.length, Array.isArray(iterated.errors));
try { new AggregateError(1); } catch (error) { console.log("non-iterable", error.constructor.name); }

// No message means no own `message`, exactly as for every other error kind; `errors` is carried
// whatever was passed, so a handler can read `error.errors.length` without checking first.
const bare = new AggregateError([]);
console.log("no-message", JSON.stringify(bare.message), bare.errors.length, JSON.stringify(Object.getOwnPropertyNames(bare)));
console.log("cause", new AggregateError([], "m", { cause: 7 }).cause);

// Called rather than constructed, which every error constructor answers the same way -- and
// which is why `throw Error(m)` is as common in real code as `throw new Error(m)`.
console.log("called", AggregateError([1], "c").errors.length, Error("x").message, TypeError("y").name);

Promise.any([Promise.reject(new Error("x")), Promise.reject(new Error("y"))]).catch((error) => {
  console.log("any", error.constructor.name, error.message, error.errors.map((reason) => reason.message).join(""), error instanceof AggregateError);
});
Promise.any([]).catch((error) => console.log("any-empty", error.constructor.name, error.errors.length));
