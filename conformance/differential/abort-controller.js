// `AbortController` and `AbortSignal`: cancellation, which every modern asynchronous API takes
// as an argument.
//
// Neither existed, so `new AbortController()` was a `ReferenceError` -- which is what stopped
// the abort block and the whole IxJS block of conformance/thirdparty/async-concurrency.
//
// A signal is an ordinary object: everything it does is state plus three methods, and nothing
// about it needs the value representation to know it exists. What it is *not* is constructable
// -- a signal comes from a controller or from one of the statics and never from a caller, which
// is what keeps cancellation one-directional.
//
// One difference from node is asserted rather than hidden: node's default reason is a
// `DOMException`, a type this realm does not have, so the constructor's name differs. Its `name`
// is `"AbortError"`, which is the property every program actually tests.
const controller = new AbortController();
const seen = [];
controller.signal.addEventListener("abort", (event) => seen.push(`${event.type}:${controller.signal.reason}`));
console.log("before", controller.signal.aborted, controller.signal.reason);
console.log("statics", typeof AbortSignal.abort, typeof AbortSignal.timeout, typeof AbortSignal.any);
controller.abort("why");
console.log("after", controller.signal.aborted, controller.signal.reason, seen.join(","));

// Idempotent, which is what makes `controller.abort()` safe from a `finally` and from a timeout
// that has already fired.
controller.abort("again");
console.log("idempotent", controller.signal.reason, seen.length);

const defaulted = new AbortController();
defaulted.abort();
console.log("default-reason", defaulted.signal.reason.name);
try { defaulted.signal.throwIfAborted(); } catch (error) { console.log("throwIfAborted", error.name); }

const already = AbortSignal.abort("pre");
console.log("static-abort", already.aborted, already.reason);

// Every member is non-enumerable, as node's are: a signal serialises as `{}`.
console.log("shape", Object.keys(controller).length, Object.keys(controller.signal).length, JSON.stringify(controller.signal));

const removed = new AbortController();
const listener = () => seen.push("never");
removed.signal.addEventListener("abort", listener);
removed.signal.removeEventListener("abort", listener);
removed.abort();
console.log("removed", seen.length);

console.log("any-already", AbortSignal.any([new AbortController().signal, already]).aborted);
const first = new AbortController();
const second = new AbortController();
const either = AbortSignal.any([first.signal, second.signal]);
second.abort("second");
console.log("any-later", either.aborted, either.reason);

const timed = AbortSignal.timeout(5);
console.log("timeout-before", timed.aborted);
setTimeout(() => console.log("timeout-after", timed.aborted, timed.reason.name), 10);

try { new AbortSignal(); } catch (error) { console.log("illegal-constructor", error.constructor.name); }

// `TextEncoder`, whose decoder half was already here. Rust strings are UTF-8 already, so the
// encoder is the identity on the bytes -- which is the whole of what it is defined to do.
const encoder = new TextEncoder();
console.log("encode", encoder.encoding, Array.from(encoder.encode("hi€")).join(","));
