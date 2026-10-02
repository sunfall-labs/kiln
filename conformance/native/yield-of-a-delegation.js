// A `yield*` completes by resuming at the instruction after it, and a `yield` resumes by its own
// position -- so in `yield yield* inner()`, where the `yield` is that very instruction, the two
// resume points were one number: the native build emitted a duplicate `case` and a redefined
// label and did not compile, in every profile. The release build's source DCE writes this shape
// from `const r = yield* inner(); yield r;` (docs/issues/parallel-builtins issue 18).
function* inner() { const sent = yield 1; yield sent; return "inner done"; }
function* outer() { yield yield* inner(); return "outer done"; }
console.log([...outer()]);

const g = outer();
console.log(JSON.stringify([g.next("ignored"), g.next("sent"), g.next(), g.next("last"), g.next()]));

function* twice() { yield (yield* inner()) + "!" ; yield yield* inner(); }
console.log([...twice()]);

const early = outer();
early.next();
console.log(JSON.stringify([early.return("stopped"), early.next()]));
