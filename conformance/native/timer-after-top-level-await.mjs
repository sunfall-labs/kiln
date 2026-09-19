// Work armed by the continuation of a top-level await still runs: the interpreter stopped its
// loop the moment the entry's await settled and dropped this timer (fixed 2026-09-19).
await null;
setTimeout(() => console.log("timer after await"), 0);
process.nextTick(() => console.log("tick after await"));
Promise.resolve().then(() => console.log("promise after await"));
console.log("continuation");
