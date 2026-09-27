// What `node:worker_threads` refuses rather than answers (realms-and-concurrency issue 02).
//
// WHY A PROBE FOR THINGS THAT REFUSE. Until 2026-09-26 each of these answered: the fallback
// `BroadcastChannel` accepted messages and never delivered them, the fallback `MessageChannel` had
// two empty objects for ports, `moveMessagePortToContext` handed back its argument, and
// `markAsUncloneable` accepted a mark that nothing honoured. Every one was a silent wrong answer.
// Each now refuses by name, and this probe is the record, so that nobody "fixes" one back into an
// answer. `Worker`, `BroadcastChannel` and ports across realms are real since issue 06
// (`worker-threads-*.cjs` beside this); what is left here still needs `vm` contexts or clone marks.
//
// The channel stand-ins apply only where the realm has no web messaging globals; in Node mode the
// real in-realm channels are exported, so the probe says which case it is in.
const wt = require("node:worker_threads");

function attempt(label, fn) {
  try {
    fn();
    console.log(label, "answered");
  } catch (error) {
    console.log(label, "refused:", error.message);
  }
}

const webMessaging = typeof globalThis.MessageChannel === "function";
console.log("web messaging globals:", webMessaging);
if (!webMessaging) {
  attempt("MessageChannel", () => new wt.MessageChannel());
  attempt("MessagePort", () => new wt.MessagePort());
  attempt("BroadcastChannel", () => new wt.BroadcastChannel("probe"));
}
attempt("markAsUncloneable", () => wt.markAsUncloneable({}));
attempt("moveMessagePortToContext", () => wt.moveMessagePortToContext({}, {}));
attempt("markAsUntransferable", () => wt.markAsUntransferable({}));
