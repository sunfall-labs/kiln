// What `require("node:perf_hooks")` publishes, and what functions answer.
const perfHooks = require("node:perf_hooks");

const names = Object.getOwnPropertyNames(perfHooks).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof perfHooks[name]);
}

console.log("default self-link:", perfHooks.default === perfHooks);
console.log("performance is obj:", typeof perfHooks.performance === "object");
console.log("performance same global:", perfHooks.performance === globalThis.performance);
console.log("performance.now is fn:", typeof perfHooks.performance.now === "function");
console.log("performance.now is number:", typeof perfHooks.performance.now() === "number");
console.log("constants is obj:", typeof perfHooks.constants === "object");
console.log("constants GC_MAJOR:", perfHooks.constants.NODE_PERFORMANCE_GC_MAJOR);
console.log("Performance is fn:", typeof perfHooks.Performance === "function");
console.log("PerformanceEntry is fn:", typeof perfHooks.PerformanceEntry === "function");
console.log("PerformanceMark is fn:", typeof perfHooks.PerformanceMark === "function");
console.log("PerformanceMeasure is fn:", typeof perfHooks.PerformanceMeasure === "function");
console.log("PerformanceObserver is fn:", typeof perfHooks.PerformanceObserver === "function");
console.log("PerformanceObserverEntryList is fn:", typeof perfHooks.PerformanceObserverEntryList === "function");
console.log("createHistogram is fn:", typeof perfHooks.createHistogram === "function");
console.log("monitorEventLoopDelay is fn:", typeof perfHooks.monitorEventLoopDelay === "function");
console.log("eventLoopUtilization is fn:", typeof perfHooks.eventLoopUtilization === "function");
console.log("timerify is fn:", typeof perfHooks.timerify === "function");

const entry = new perfHooks.PerformanceEntry("e1", "mark", 100, 50);
console.log("entry:", entry.name, entry.entryType, entry.startTime, entry.duration);
const json = entry.toJSON();
console.log("entry JSON:", json.name, json.entryType, json.startTime, json.duration);

const mark = new perfHooks.PerformanceMark("m1", { detail: "d1", startTime: 200 });
console.log("mark:", mark.name, mark.entryType, mark.startTime, mark.duration, mark.detail);

const measure = new perfHooks.PerformanceMeasure("meas1", { detail: "d2", startTime: 10, duration: 40 });
console.log("measure:", measure.name, measure.entryType, measure.startTime, measure.duration, measure.detail);

const list = new perfHooks.PerformanceObserverEntryList([entry, mark]);
console.log("list entries:", list.getEntries().length, list.getEntriesByName("e1").length, list.getEntriesByType("mark").length);

const hist = perfHooks.createHistogram();
console.log("histogram min/max:", hist.min, hist.max);

const delay = perfHooks.monitorEventLoopDelay();
console.log("delay min/max:", delay.min, delay.max);

const elu = perfHooks.eventLoopUtilization();
console.log("elu:", elu.idle, elu.active, elu.utilization);
