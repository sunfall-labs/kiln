// `performance` reads the realm's clock, and the realm has only the virtual one (ADR 0015).
//
// Nothing here prints an instant taken from the host, because there is no host instant to take:
// the clock starts at the epoch and moves only when a timer says time has passed. That is what
// makes these numbers writable at all -- `performance.now()` is a *measurement* everywhere else,
// and a measurement cannot be an expectation. Here it is arithmetic.

console.log(typeof performance, typeof performance.now);

// Twice in the same turn, with nothing in between: no time has passed because nothing has
// happened, so both reads are the same number rather than two samples of a running counter.
const first = performance.now();
const second = performance.now();
console.log(first, second, first === second);

// The same clock `Date.now` reads, which is the whole of the decision: two APIs, one clock, so
// a program cannot learn from their difference something neither of them was told.
console.log(performance.now() === Date.now());

// The spelling feature detection actually uses has to find the same value the bare name does.
console.log(globalThis.performance === performance, typeof globalThis.performance);

// And time passes when a timer says it has, which is the only way it passes here. A duration
// measured across `setTimeout` is therefore exact rather than approximate.
const start = performance.now();
setTimeout(() => {
  const elapsed = performance.now() - start;
  console.log(elapsed, elapsed === 50);
}, 50);
