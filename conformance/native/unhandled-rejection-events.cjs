// `unhandledRejection` sees (reason, promise) once each checkpoint ends -- ticks and promise jobs
// first -- and suppresses the default; what its handlers queue runs before the next report; a
// promise handled after it was reported is `rejectionHandled`; each macrotask ends with its own
// report, before the next timer or immediate.
const log = (...a) => console.log(...a);
process.on("unhandledRejection", (reason, promise) => {
  log("unhandledRejection", String(reason));
  process.nextTick(() => log("tick from handler", String(reason)));
  Promise.resolve().then(() => log("job from handler", String(reason)));
  if (reason === "first") {
    promise.catch(() => log("caught inside handler"));
    Promise.reject("second");
  }
});
process.on("rejectionHandled", (promise) => log("rejectionHandled"));
Promise.reject("first");
const handledInTime = Promise.reject("in time");
process.nextTick(() => { log("tick"); handledInTime.catch(() => log("caught in time")); });
setTimeout(() => {
  log("timer 1");
  Promise.reject("from timer");
  process.nextTick(() => log("tick after timer 1"));
}, 0);
setTimeout(() => {
  log("timer 2");
  setImmediate(() => { log("immediate"); Promise.reject("from immediate"); });
  setImmediate(() => log("immediate 2"));
}, 0);
(async () => { await null; throw "async"; })();
