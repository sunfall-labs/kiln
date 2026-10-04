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

const dispatch = new AbortController();
const order = [];
let selfRuns = 0;
function selfRemoving() {
  order.push(`self-${++selfRuns}`);
  if (selfRuns === 1) {
    dispatch.signal.removeEventListener("abort", selfRemoving);
    dispatch.signal.addEventListener("abort", selfRemoving);
  }
}
dispatch.signal.addEventListener("abort", () => {
  order.push("first");
  dispatch.signal.removeEventListener("abort", removedListener);
  dispatch.signal.addEventListener("abort", () => order.push("late"));
});
let onceAgain = false;
const once = () => {
  order.push("once");
  if (!onceAgain) {
    onceAgain = true;
    dispatch.signal.addEventListener("abort", once, { once: true });
  }
};
dispatch.signal.addEventListener("abort", once, { once: true });
const removedListener = () => order.push("removed");
dispatch.signal.addEventListener("abort", removedListener);
dispatch.signal.addEventListener("abort", (event) => {
  order.push(event.target === dispatch.signal ? "target" : "wrong-target");
});
dispatch.signal.addEventListener("abort", selfRemoving);
dispatch.abort();
dispatch.abort();
console.log("dispatch-mutation", order.join(","));

const singleDispatch = new AbortController();
const singleOrder = [];
singleDispatch.signal.addEventListener("abort", () => {
  singleOrder.push("first");
  singleDispatch.signal.addEventListener("abort", () => singleOrder.push("late"));
});
singleDispatch.abort();
console.log("single-dispatch-add", singleOrder.join(","));
