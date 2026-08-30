// The event loop: microtasks, timers, and the order they interleave in.
//
// The clock is virtual on both sides -- it advances to whatever is due next rather than waiting
// for it -- so the delays here name an *order*, not a duration. That is the whole of what a
// program can observe about a timer, and it is what the two engines have to agree on.

const log: string[] = [];
setTimeout(() => log.push("later"), 40);
setTimeout(() => log.push("zero"), 0);
setTimeout(() => log.push("also zero"), 0);
queueMicrotask(() => log.push("micro"));
log.push("sync");
setTimeout(() => console.log(log.join(",")), 60);

// Microtasks drain completely before any timer, and again after each one -- which is why a job
// queued from inside a timer runs before the next timer, however early that one was due.
const order: string[] = [];
setTimeout(() => {
  order.push("first timer");
  queueMicrotask(() => order.push("micro from timer"));
}, 10);
setTimeout(() => order.push("second timer"), 20);
setTimeout(() => console.log(order.join(",")), 30);

// Observable time is the same paused Realm clock that orders timers. Advancing to this callback
// must advance `Date.now()` by exactly its virtual delay, without sleeping in wall-clock time.
const clockStart = Date.now();
setTimeout(() => console.log("clock", Date.now() - clockStart), 25);

// A microtask that queues another still drains before anything else.
const chain: number[] = [];
queueMicrotask(() => { chain.push(1); queueMicrotask(() => chain.push(2)); });
setTimeout(() => console.log(chain.join(",")), 0);

// Arguments after the delay reach the callback, which is how a timer carries state without a
// closure. A missing or negative delay is "as soon as possible", not "before what is queued".
setTimeout((a: string, b: number) => console.log("args", a, b), 10, "x", 2);
setTimeout(() => console.log("no delay"));
setTimeout(() => console.log("negative"), -5);

// A cleared timer never runs, clearing an id that is gone is harmless, and the two clear
// functions are interchangeable however the timer was made.
const cancelled = setTimeout(() => console.log("never"), 10);
clearTimeout(cancelled);
clearTimeout(cancelled);
clearTimeout(9999);
clearInterval(setTimeout(() => console.log("never either"), 10));

// An interval reschedules itself and can cancel itself from inside.
let ticks = 0;
const interval = setInterval(() => {
  ticks += 1;
  if (ticks === 3) {
    clearInterval(interval);
    console.log("ticks", ticks);
  }
}, 20);

// Every id is different. `typeof` is deliberately not asked: the language says a number and node
// hands back a `Timeout` object, so the two disagree for reasons that have nothing to do with the
// backend.
// Each of the five is named for the program's spelling of it. `setTimeout` is a derivation trap
// on the interpreter side -- `Set` is a type name that is not this one's type -- and node adds its
// own symbol property to the function, so this is checked between the engines rather than against
// node.
console.log(setTimeout, setInterval, clearTimeout, clearInterval, queueMicrotask);
console.log(setTimeout === setInterval, clearTimeout === clearInterval);

const first = setTimeout(() => {}, 100);
const second = setTimeout(() => {}, 100);
console.log(first !== second);
clearTimeout(first);
clearTimeout(second);

// A promise resolved from a timer still hands its value on through the microtask queue.
new Promise<string>((resolve) => setTimeout(() => resolve("awaited"), 10))
  .then((value) => console.log("promise", value));
