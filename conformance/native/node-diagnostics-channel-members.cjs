// What `require("node:diagnostics_channel")` publishes, and what functions answer.
const dc = require("node:diagnostics_channel");

const names = Object.getOwnPropertyNames(dc).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof dc[name]);
}

console.log("default self-link", dc.default === dc);
console.log("Channel is fn:", typeof dc.Channel === "function");
console.log("TracingChannel is fn:", typeof dc.TracingChannel === "function");

// Channel creation and registry identity
const c1 = dc.channel("test.chan");
const c2 = dc.channel("test.chan");
console.log("channel identity:", c1 === c2);
console.log("channel name:", c1.name);

// Subscription and publishing
const received = [];
const sub = (data, name) => {
  received.push(name + ":" + data.val);
};

console.log("hasSubscribers before:", dc.hasSubscribers("test.chan"), c1.hasSubscribers);
dc.subscribe("test.chan", sub);
console.log("hasSubscribers during:", dc.hasSubscribers("test.chan"), c1.hasSubscribers);

c1.publish({ val: 42 });
c1.publish({ val: 99 });

dc.unsubscribe("test.chan", sub);
console.log("hasSubscribers after:", dc.hasSubscribers("test.chan"), c1.hasSubscribers);
console.log("received:", received.join(","));

// Tracing channel
const tc = dc.tracingChannel("tracer");
console.log("tc subchannels:", typeof tc.start, typeof tc.end, typeof tc.asyncStart, typeof tc.asyncEnd, typeof tc.error);
console.log("tc hasSubscribers:", tc.hasSubscribers);

let traceRan = false;
tc.start.runStores({}, function(x) {
  traceRan = this.flag && x === 42;
}, { flag: true }, 42);
console.log("traceRan:", traceRan);

const syncRes = tc.traceSync((a, b) => a + b, {}, null, 10, 20);
console.log("syncRes:", syncRes);
