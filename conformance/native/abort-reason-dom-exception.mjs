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
// node does not let this timer keep the process alive; a referenced one waits for it here.
const wait = setTimeout(() => {}, 1000);
timed.addEventListener("abort", () => { console.log("timeout", describe(timed.reason)); clearTimeout(wait); });
