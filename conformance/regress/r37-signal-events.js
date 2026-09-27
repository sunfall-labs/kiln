// `process.on(signal)` (realms issue 23): a listener turns a signal into an event, emitted from
// the poll phase of the loop -- after the script's ticks and promise jobs, before its immediates
// -- with `(name, number)` and `process` as `this`, one event per arrival. A signal that cannot be
// caught throws from `on` and adds nothing; `process.kill` checks its arguments as node does.
// Every signal sent here has a listener when it arrives, so the runner is never at risk.
const log = [];
const say = (line) => { log.push(line); console.log(line); };

try { process.on("SIGKILL", () => {}); say("SIGKILL listener added"); }
catch (error) { say(`SIGKILL: ${error.name} ${error.code} ${error.errno} ${error.syscall} ${error.message}`); }
say(`SIGKILL listeners ${process.listenerCount("SIGKILL")}`);
try { process.on("SIGSTOP", () => {}); } catch (error) { say(`SIGSTOP: ${error.message}`); }
try { process.kill(process.pid, "SIGNOPE"); } catch (error) { say(`unknown: ${error.name} ${error.code} ${error.message}`); }
try { process.kill("abc"); } catch (error) { say(`pid: ${error.name} ${error.code} ${error.message}`); }
try { process.kill(process.pid, 1234); } catch (error) { say(`bad number: ${error.name} ${error.code} ${error.errno} ${error.syscall} ${error.message}`); }
try { process.kill(4194304 + 12345, 0); } catch (error) { say(`no such process: ${error.code} ${error.errno} ${error.syscall} ${error.message}`); }
say(`kill 0 answers ${process.kill(process.pid, 0)}`);

let usr2 = 0;
process.on("SIGUSR2", function (name, number) {
  usr2 += 1;
  say(`SIGUSR2 #${usr2}: ${name} ${typeof number} ${number > 0} this=${this === process}`);
  process.nextTick(() => say(`tick inside SIGUSR2 #${usr2}`));
  Promise.resolve().then(() => say(`promise job inside SIGUSR2 #${usr2}`));
});
process.once("SIGHUP", (name) => say(`once ${name}, listeners left ${process.listenerCount("SIGHUP")}`));
process.on("SIGHUP", (name) => say(`on ${name}`));

say(`kill answers ${process.kill(process.pid, "SIGUSR2")}`);
process.kill(process.pid, "SIGHUP");
say("script end");
process.nextTick(() => say("tick"));
Promise.resolve().then(() => say("promise job"));
setImmediate(() => {
  say("immediate");
  process.kill(process.pid, "SIGUSR2");
  process.kill(process.pid, "SIGHUP");
  setImmediate(() => {
    say("second immediate");
    process.removeAllListeners("SIGHUP");
    process.removeAllListeners("SIGUSR2");
  });
});
