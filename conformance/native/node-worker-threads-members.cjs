// What `require("node:worker_threads")` publishes, and what functions answer.
const wt = require("node:worker_threads");

const names = Object.getOwnPropertyNames(wt).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof wt[name]);
}

console.log("default self-link:", wt.default === wt);
console.log("Worker is fn:", typeof wt.Worker === "function");
console.log("isMainThread is bool:", typeof wt.isMainThread === "boolean");
console.log("isMainThread:", wt.isMainThread);
console.log("parentPort is null:", wt.parentPort === null);
console.log("threadId is num:", typeof wt.threadId === "number");
console.log("threadId:", wt.threadId);
console.log("workerData is null:", wt.workerData === null);
console.log("SHARE_ENV is sym:", typeof wt.SHARE_ENV === "symbol");
console.log("SHARE_ENV same global:", wt.SHARE_ENV === Symbol.for("nodejs.worker_threads.SHARE_ENV"));
console.log("MessageChannel is fn:", typeof wt.MessageChannel === "function");
console.log("MessagePort is fn:", typeof wt.MessagePort === "function");
console.log("BroadcastChannel is fn:", typeof wt.BroadcastChannel === "function");
console.log("setEnvironmentData is fn:", typeof wt.setEnvironmentData === "function");
console.log("getEnvironmentData is fn:", typeof wt.getEnvironmentData === "function");

wt.setEnvironmentData("probe_key", "probe_val");
console.log("envData read:", wt.getEnvironmentData("probe_key"));

let threw = false;
try {
  new wt.Worker("worker.js");
} catch (e) {
  threw = true;
  console.log("Worker throws:", threw, e.message.includes("worker"));
}
