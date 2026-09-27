// A reported rejection handled later with no `rejectionHandled` listener is node's
// `PromiseRejectionHandledWarning`, emitted from a tick as a `warning` event.
process.on("warning", (warning) => console.log("warning", warning.name, warning.message));
process.on("unhandledRejection", (reason) => console.log("unhandledRejection", reason));
const late = Promise.reject(1);
setTimeout(() => {
  late.catch(() => console.log("caught"));
  console.log("handled late");
}, 0);
setTimeout(() => console.log("after"), 5);
