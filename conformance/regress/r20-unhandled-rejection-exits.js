// An unhandled rejection is node's default `--unhandled-rejections=throw`: reported once the
// checkpoint that left it unhandled is over (ticks and promise jobs first), then the program
// ends as for an uncaught exception, exit status 1, before any timer runs.
console.log("start");
const handledInTime = Promise.reject(new Error("handled on the next line"));
handledInTime.catch(() => console.log("caught in time"));
const handledByTick = Promise.reject(new Error("handled from a tick"));
process.nextTick(() => handledByTick.catch(() => console.log("caught from a tick")));
Promise.reject(new Error("boom"));
setTimeout(() => console.log("never"), 0);
process.nextTick(() => console.log("tick"));
Promise.resolve().then(() => console.log("then"));
