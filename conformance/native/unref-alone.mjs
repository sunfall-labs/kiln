// Nothing referenced is left once the module has run, so none of these fires and the program
// ends at once: a timeout, an interval, an immediate and `AbortSignal.timeout`.
setTimeout(() => console.log("unref'd timeout fired"), 1).unref();
setInterval(() => console.log("unref'd interval fired"), 1).unref();
setImmediate(() => console.log("unref'd immediate fired")).unref();
const signal = AbortSignal.timeout(1);
signal.addEventListener("abort", () => console.log("signal fired"));
console.log("sync done");
Promise.resolve().then(() => console.log("microtask"));
process.nextTick(() => console.log("tick"));
