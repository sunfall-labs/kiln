// A built-in constructor's own `length`, which this backend refuses rather than guessing.
//
// WHY A PROBE FOR SOMETHING THAT REFUSES. A refusal is a decision, and a decision nobody wrote
// down is one the next person "fixes" by answering `undefined` -- which is what this member did
// until 2026-09-02, silently, on eleven constructors. `Array.length` is 1 in the interpreter and
// read as `undefined` in the compiled backend, because `length`, `name` and `prototype` belong to
// every callable and so are deliberately outside the shared surface table (ADR 0043) -- which put
// them outside the rule that turns an unbuilt member into a refusal.
//
// The decision is to refuse. A built-in function's arity is written down in neither engine's
// tables, a guess is a wrong answer, and nothing this repository bundles feature-detects on it.
// If somebody gives the backend arities, this probe starts *matching* and that is the right
// outcome; what it must never do again is print `undefined` while the interpreter prints 1.
//
// scripts/native.sh counts this as `refused`, which is the backlog and not a failure. It becomes a
// divergence the moment the two engines disagree about an answer rather than about a refusal.
console.log("before", typeof Array);
console.log("Array.length", Array.length);
console.log("unreachable");
