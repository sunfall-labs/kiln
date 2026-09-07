// The order of the three job lanes -- process.nextTick, the promise queue, and the host work
// that reaches the program as a timer or an I/O completion -- as node orders them.
//
// The promise queue is the prelude's own now (Phase 6.3): an `await` resumption and a `then`
// reaction are entries in an array the generated C owns, and the realm drains that array by
// calling back into it rather than receiving one work item per job. Ordering is the whole risk
// in that move, and one rule is easy to get wrong in either direction: a microtask checkpoint
// runs the promise queue *to exhaustion*, so a `nextTick` queued from inside a promise job
// waits for every promise job already queued -- `then-1`, `nextTick(t)`, `then-2` prints
// `then-2` before `t`, not after `then-1`.
//
// A `.cjs` probe, so that both engines get the Node Adapter and `fs.promises` is a real host
// completion rather than a resolved promise. `setImmediate` is left out: the native backend does
// not implement it yet (`setImmediate(f)` throws a TypeError there), which is its own gap.
//
// The phases are sequential -- each one is entered by the previous one's callback -- so nothing
// here races two host operations against each other and every line is determined by the lane
// rules rather than by which of two completions the operating system delivered first.
"use strict";
const fs = require("fs");

const log = [];
const say = (text) => log.push(text);

function phase(name, schedule) {
  say("== " + name);
  return new Promise((done) => schedule(done));
}

async function main() {
  // Everything queued from synchronous code: the tick lane runs first and whole, then the
  // promise queue whole. `micro-1-tick` is the rule above -- queued from a promise job, it runs
  // after `then-2` and `await-2`, in the next pass over the tick lane.
  await phase("sync", (done) => {
    process.nextTick(() => {
      say("tick-1");
      process.nextTick(() => say("tick-1-nested"));
      Promise.resolve().then(() => say("tick-1-promise"));
    });
    queueMicrotask(() => {
      say("micro-1");
      process.nextTick(() => say("micro-1-tick"));
    });
    Promise.resolve()
      .then(() => say("then-1"))
      .then(() => say("then-2"));
    (async () => {
      await null;
      say("await-1");
      await Promise.resolve();
      say("await-2");
    })();
    process.nextTick(() => {
      say("tick-2");
      done();
    });
  });

  // A timer is a turn of its own, and the checkpoint after it drains both lanes before the next
  // timer runs.
  await phase("timer", (done) => {
    setTimeout(() => {
      say("timeout-0");
      process.nextTick(() => say("timeout-0-tick"));
      queueMicrotask(() => say("timeout-0-micro"));
      Promise.resolve().then(() => say("timeout-0-then"));
      setTimeout(() => {
        say("timeout-1");
        done();
      }, 0);
    }, 0);
  });

  // A real host completion: the promise it settles runs its reactions in the same checkpoint,
  // and the `nextTick` it queues waits for them.
  await phase("readFile", (done) => {
    fs.promises.readFile(__filename, "utf8").then((text) => {
      say("read:" + (text.indexOf("job lanes") >= 0));
      process.nextTick(() => say("read-tick"));
      queueMicrotask(() => say("read-micro"));
      Promise.resolve()
        .then(() => say("read-then-1"))
        .then(() => {
          say("read-then-2");
          done();
        });
    });
  });

  // One more turn, so the last pass over the tick lane is in the log rather than after the print.
  await new Promise((done) => setTimeout(done, 1));
  say("done");
  console.log(log.join("\n"));
}

main();
