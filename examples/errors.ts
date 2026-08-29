// Errors, `new`, and `typeof`. The first conformance probe kiln passes exercises exactly this.
const inner = new Error("inner");
const outer = new Error("outer", { cause: inner });

console.log("message", outer.message);
console.log("name", outer.name);
// A header line and nothing more: a release build strips unwind tables, so real frames are not
// obtainable. This is the shape V8 produces with Error.stackTraceLimit = 0.
console.log("stack", outer.stack);
// `cause` is carried by identity, not by copying.
console.log("cause is inner", outer.cause === inner);
// An error with no message has a bare header rather than a trailing colon.
console.log("bare", new Error().stack);

console.log("typeof", typeof Error, typeof outer, typeof outer.message, typeof outer.missing);
console.log("captureStackTrace", typeof Error.captureStackTrace);
