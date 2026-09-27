// Issue 14 of `.scratch/realms-and-concurrency`, what remained open after r17: where a port's
// delivery sits among the event loop's phases, and the event's own symbol-keyed slots.
//
// Node delivers a port's messages in libuv's poll phase: after the timers due when a turn begins,
// before the turn's immediates. So an immediate scheduled before the first `postMessage` still
// runs after the messages, and one scheduled by a timer callback runs after the message that
// callback posted. An immediate that posts runs the rest of its check phase first, and an
// immediate it schedules waits for the next turn, behind the message. Output recorded from node
// 26.8.1.
const log = (line) => console.log(line);

// ---- the event's slots ---------------------------------------------------------------------

const slots = (event) => Object.getOwnPropertySymbols(event).map((key) => {
  const value = event[key];
  return key.description + "=" + (value !== null && typeof value === "object" ? value.constructor.name : String(value));
}).join(" ");
const event = new Event("x");
log("keys " + Reflect.ownKeys(event).length + " " + JSON.stringify(Object.keys(event)) + " " + Object.getOwnPropertySymbols({ ...event }).length);
log("event " + slots(event));
log("custom " + slots(new CustomEvent("c", { detail: 1 })));
const target = new EventTarget();
target.addEventListener("x", (seen) => log("during " + slots(seen)), { passive: true });
target.dispatchEvent(event);
log("after " + slots(event));
const descriptor = Object.getOwnPropertyDescriptor(event, Object.getOwnPropertySymbols(event)[0]);
log("descriptor " + descriptor.writable + " " + descriptor.enumerable + " " + descriptor.configurable);
event[Object.getOwnPropertySymbols(event)[0]] = "renamed";
log("type is the slot " + event.type);
try {
  Object.getOwnPropertyDescriptor(Event.prototype, "type").get.call({});
} catch (error) {
  log("not an event " + error.name + " " + error.code);
}

// ---- the poll phase ------------------------------------------------------------------------

const { port1, port2 } = new MessageChannel();
port2.onmessage = (message) => {
  log("message " + message.data + " " + slots(message));
  process.nextTick(() => log("tick after " + message.data));
  if (message.data === "a2") {
    setTimeout(() => {
      log("timeout");
      setImmediate(() => log("immediate from the timeout"));
      port1.postMessage("b1");
    }, 5);
  }
  if (message.data === "b1") {
    setImmediate(() => {
      log("immediate 1");
      setImmediate(() => log("immediate from immediate 1"));
      port1.postMessage("c1");
    });
    setImmediate(() => log("immediate 2"));
  }
  if (message.data === "c1") port2.close();
};
setImmediate(() => log("immediate before the first post"));
port1.postMessage("a1");
setImmediate(() => log("immediate after the first post"));
port1.postMessage("a2");
Promise.resolve().then(() => log("promise job of the module"));
