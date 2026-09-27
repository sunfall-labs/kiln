// `process.on(signal)` (realms issue 23): a listened-for signal is an event from the poll phase --
// after the script's ticks and promise jobs, before its immediates -- and not the end of the
// program. A SIGTERM listener that finishes its work and calls `process.exit(0)` ends the
// program with 0, however much else is still scheduled.
const log = (...a) => console.log(...a);
process.on("SIGUSR2", (name, number) => {
  log("event", name, typeof number);
  process.nextTick(() => log("tick in", name));
  Promise.resolve().then(() => log("job in", name));
});
process.on("SIGTERM", (name) => {
  log("event", name, "draining");
  setTimeout(() => {
    log("drained");
    process.exit(0);
    log("not reached");
  }, 5);
});
const keepAlive = setInterval(() => log("interval"), 60000);
log("kill", process.kill(process.pid, "SIGUSR2"));
process.nextTick(() => log("tick"));
Promise.resolve().then(() => log("job"));
setImmediate(() => {
  log("immediate");
  process.kill(process.pid, "SIGTERM");
});
log("script end");
void keepAlive;
