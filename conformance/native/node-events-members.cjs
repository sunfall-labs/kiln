const events = require("node:events");
const EventEmitter = events.EventEmitter;

// Constructor identity and properties
console.log("type:", typeof EventEmitter);
console.log("name:", EventEmitter.name);
console.log("default:", events.default === EventEmitter);
console.log("self:", EventEmitter.EventEmitter === EventEmitter);
console.log("defaultMaxListeners:", EventEmitter.defaultMaxListeners);

// Instance methods and aliasing
const ee = new EventEmitter();
console.log("onAlias:", ee.addListener === ee.on);
console.log("offAlias:", ee.removeListener === ee.off);

// Listener tracking and events
const eventsNew = [];
ee.on("newListener", (ev) => {
  eventsNew.push(ev);
});

let calls = 0;
function onData(a, b) {
  calls += 1;
  console.log("data:", a, b);
}

ee.on("data", onData);
ee.once("data", (a, b) => {
  console.log("once-data:", a, b);
});

console.log("newListenerLog:", eventsNew.join(","));
console.log("listenerCount:", ee.listenerCount("data"));
console.log("eventNames:", ee.eventNames().sort().join(","));

// Emitting events
console.log("emit1:", ee.emit("data", "x", 1));
console.log("emit2:", ee.emit("data", "y", 2));
console.log("emitCalls:", calls);
console.log("postOnceCount:", ee.listenerCount("data"));

// Remove listener
ee.removeListener("data", onData);
console.log("afterRemoveCount:", ee.listenerCount("data"));
console.log("emitEmpty:", ee.emit("data", "z", 3));

// Max listeners
console.log("defaultMax:", ee.getMaxListeners());
ee.setMaxListeners(25);
console.log("customMax:", ee.getMaxListeners());

// Error handling
let caught = false;
ee.on("error", (err) => {
  caught = true;
  console.log("caughtErr:", err.message);
});
ee.emit("error", new Error("boom"));
console.log("errHandled:", caught);

