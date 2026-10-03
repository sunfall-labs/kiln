// An AbortSignal's default reasons are platform exceptions, as in node: `abort()` gives an
// AbortError DOMException (code 20) and `AbortSignal.timeout` a TimeoutError (code 23).
const describe = (reason) => [reason instanceof DOMException, reason instanceof Error, reason.name,
  reason.code, Object.prototype.toString.call(reason), Object.keys(reason).length].join(" ");

const controller = new AbortController();
controller.abort();
console.log("controller", describe(controller.signal.reason));
console.log("static", describe(AbortSignal.abort().reason));
console.log("explicit", AbortSignal.abort("why").reason);
const timed = AbortSignal.timeout(1);
timed.addEventListener("abort", () => console.log("timeout", describe(timed.reason)));
// The timeout's own timer is unreferenced, in node and here, so it takes something else to keep
// the program running until it fires.
setTimeout(() => {}, 50);
