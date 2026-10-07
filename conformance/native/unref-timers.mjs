// `unref()` on a timer, an interval or an immediate, and `AbortSignal.timeout`, which is
// unreferenced in node: none of them keeps the program alive, `ref()` undoes it, and while
// something else does keep the loop going they still run when their turn comes. The one at 1000
// ms would print last if it were waited for.
const t = setTimeout(() => console.log("unref'd timeout 1000 fired"), 1000);
console.log("handle", typeof t, t.hasRef());
console.log("unref", t.unref() === t, t.hasRef());
console.log("ref", t.ref() === t, t.hasRef());
console.log("unref again", t.unref().hasRef());
console.log("unref twice", t.unref().hasRef());

setTimeout(() => console.log("unref().ref() timeout fired"), 10).unref().ref();

let ticks = 0;
const interval = setInterval(() => ticks++, 10);
interval.unref();
console.log("interval", interval.hasRef());

setTimeout(() => console.log("unref'd timeout 20 fired while the loop is alive"), 20).unref();
setImmediate(() => console.log("unref'd immediate ran while the loop is alive")).unref();

const signal = AbortSignal.timeout(30);
signal.addEventListener("abort", () => console.log("signal", signal.reason.name));
AbortSignal.timeout(5000).addEventListener("abort", () => console.log("5000 signal fired"));

// A referenced timer keeps the loop open past all of the above.
setTimeout(() => {
  console.log("referenced timeout 80 fired, interval ticked", ticks >= 3);
  // Cleared handles and handles of finished timers are harmless to ref and unref.
  clearTimeout(t);
  console.log("after", t.ref().hasRef(), t.unref().hasRef());
}, 80);

// The handles come from the engine-private channel, not from anything a program can find.
console.log("__kiln names", Object.getOwnPropertyNames(globalThis).filter((name) => name.startsWith("__kiln")).length);
