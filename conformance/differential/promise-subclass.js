// A subclass of a built-in inherits that built-in's statics, and a `Promise` subclass's
// factories answer promises of the subclass.
//
// `class P extends Promise {}` links the two prototypes *and* the two constructors. The second
// link was made -- `Object.getPrototypeOf(P) === Promise` was already true -- but the property
// walk stopped at it, because an intrinsic has no property store to continue into. So
// `P.resolve` and `P.all` were `undefined`, which is the shape every cancellable-promise and
// bluebird-interop layer is built on.
class P extends Promise { extra() { return "x"; } }
console.log("statics", typeof P.resolve, typeof P.all, typeof P.race, typeof P.allSettled, typeof P.reject);
class A extends Array {}
class M extends Map {}
class O extends Object {}
console.log("others", typeof A.from, typeof A.of, typeof A.isArray, typeof M.groupBy, typeof O.keys);
class B extends P {}
console.log("two-deep", typeof B.resolve);

// A promise a subclass's factory built is one of the subclass, which is the whole reason to
// subclass: without it the layer's own methods are missing from everything it hands back.
console.log("resolve", P.resolve(1) instanceof P, typeof P.resolve(1).extra);
console.log("reject", P.reject(1).catch(() => {}) instanceof P);
console.log("combinators", P.all([1]) instanceof P, P.race([1]) instanceof P, P.allSettled([1]) instanceof P, P.any([1]) instanceof P);
console.log("withResolvers", P.withResolvers().promise instanceof P);

// `then` follows the same chain, through `SpeciesConstructor`, so the methods survive a chain
// rather than living only on its first link.
const first = new P((resolve) => resolve(1));
console.log("then", first.then(() => 1) instanceof P, first.catch(() => 1) instanceof P, first.finally(() => 1) instanceof P);

// And a subclass that opts *out* by naming the intrinsic species gets the intrinsic back, which
// is the standard way a library subclasses without infecting everything downstream.
class Opted extends Promise { static get [Symbol.species]() { return Promise; } }
const opted = new Opted((resolve) => resolve(1));
console.log("opted-out", opted.then(() => 1) instanceof Opted, opted.then(() => 1) instanceof Promise);

// `Promise.resolve(p) === p` still holds for a promise of the same constructor, and a promise of
// a *different* one is wrapped rather than passed through.
const plain = Promise.resolve(7);
console.log("identity", Promise.resolve(plain) === plain, P.resolve(plain) === plain, P.resolve(plain) instanceof P);
P.resolve(plain).then((value) => console.log("wrapped-value", value));
