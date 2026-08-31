const controller = new AbortController();
const seen = [];
controller.signal.addEventListener("abort", (event) => seen.push(event.type));
console.log("before", controller.signal.aborted, controller.signal.reason);
controller.abort("why");
console.log("after", controller.signal.aborted, controller.signal.reason, seen.join(","));
controller.abort("again");
console.log("idempotent", controller.signal.reason, seen.length);

const already = AbortSignal.abort("pre");
console.log("static", already.aborted, already.reason);

const removed = new AbortController();
const listener = () => seen.push("never");
removed.signal.addEventListener("abort", listener);
removed.signal.removeEventListener("abort", listener);
removed.abort();
console.log("removed", seen.length, removed.signal.reason.name);

try { removed.signal.throwIfAborted(); } catch (error) { console.log("thrown", error.name); }
try { new AbortSignal(); } catch (error) { console.log("illegal", error.constructor.name); }

const first = new AbortController();
const second = new AbortController();
const either = AbortSignal.any([first.signal, second.signal]);
second.abort("second");
console.log("any", either.aborted, either.reason);

const timed = AbortSignal.timeout(5);
console.log("timeout-before", timed.aborted);
setTimeout(() => console.log("timeout-after", timed.aborted, timed.reason.name), 10);
