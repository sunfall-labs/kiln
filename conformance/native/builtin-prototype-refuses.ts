// `ArrayBuffer.prototype`, which this backend refuses rather than answering `undefined`.
//
// WHY A PROBE FOR SOMETHING THAT REFUSES. There is a prototype-pair table in the emitted C, and
// `ArrayBuffer` and `Symbol` were knowingly left out of it because neither has a prototype builtin
// constant. What was not intended is what the read then did: `prototype` is outside the shared
// surface table, so the rule "a name this table has no row for is `undefined`" answered for it,
// and `ArrayBuffer.prototype` and `Symbol.prototype` read `undefined` against the interpreter's
// objects. `Object.create(ArrayBuffer.prototype)` and `x instanceof` checks walk straight past.
//
// The decision is to refuse rather than to invent the pair. A prototype object with no method
// table behind it would publish nothing and answer `undefined` for each of its own members --
// twelve silent wrong answers on `ArrayBuffer.prototype` and four on `Symbol.prototype`, in place
// of the one that is here now. Answering costs a real prototype with real members, and until
// somebody builds it the refusal names the member and stops the program at the cause.
console.log("before", typeof ArrayBuffer, typeof Symbol);
console.log("ArrayBuffer.prototype", typeof ArrayBuffer.prototype);
console.log("Symbol.prototype", typeof Symbol.prototype);
console.log("unreachable");
