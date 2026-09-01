// `Math.max.length` refuses in the native backend, where the interpreter answers 2.
//
// Found by conformance/native/callable-surface.js, which asks every callable shape for every
// member a callable has. Every other member on that surface now agrees between the two engines;
// this is the one that does not, so it is the one the gate leaves out.
//
// It is a *refusal*, not a wrong answer -- the backend says it cannot answer rather than
// inventing a number -- which is why it is recorded here instead of being fixed in a hurry. The
// arity of a built-in lives in `native_length` in kiln-interpreter, a ~190-line table keyed by
// `Native` with per-variant answers (`Math.max` is 2, `Object.keys` is 1, the accessors are 0).
// The C backend has no copy of it and should not grow one by hand: a second list of 361 arities
// maintained beside the first is exactly how the `node:http` export slots rotted, silently, until
// a test was written to pin them together.
//
// So the fix wants a shared source of truth rather than a transcription -- the arity belongs
// beside the method code in the registry both sides already read, with a test pinning it to the
// interpreter's answers the way `native_builtin_slots_match_the_shared_export_table` pins the
// builtin export bodies.
//
// node:   2 1 1 2
// kiln:   2 1 1 2   (interpreter)
//         refuses   (native backend: operation=GetComputed on `length`)
console.log(Math.max.length, Object.keys.length, Object.length, parseInt.length);
