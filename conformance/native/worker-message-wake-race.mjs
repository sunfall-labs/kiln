// kiln-native: clock=wall
// A message posted to a worker that is mid-turn and about to park must keep every virtual clock
// held from the moment it is sent, not from the moment the worker next runs. The parent holds a
// long guard timer on every round; if the worker went idle on its way to park while the message
// was queued but not yet run, the parent's virtual clock jumped to the guard and printed
// "timeout" instead of the reply. The worker does a short burst of work per message so the
// window between its last turn and its park is wide, and the parent posts both one message at a
// time (the worker parks between) and in bursts (messages land while it is mid-turn).
import { Worker, isMainThread, parentPort } from "node:worker_threads";

const ROUNDS = 150;

function burn(n) {
  let x = 0;
  for (let i = 0; i < n; i++) x = (x + i * 31) % 1000003;
  return x;
}

async function main() {
  const worker = new Worker(new URL(import.meta.url));
  const inbox = [];
  let waiting = null;
  worker.on("message", (m) => {
    if (waiting) {
      const w = waiting;
      waiting = null;
      w(m);
    } else inbox.push(m);
  });
  const next = () =>
    new Promise((resolve) => {
      if (inbox.length) return resolve(inbox.shift());
      const guard = setTimeout(() => {
        waiting = null;
        resolve("timeout");
      }, 60000);
      waiting = (m) => {
        clearTimeout(guard);
        resolve(m);
      };
    });

  let bad = 0;
  for (let i = 0; i < ROUNDS; i++) {
    worker.postMessage({ n: i, work: 20000 + (i % 7) * 15000 });
    const reply = await next();
    if (reply !== "ack " + i) bad++;
  }
  console.log("single rounds, wrong replies:", bad);

  bad = 0;
  for (let i = 0; i < 30; i++) {
    for (let k = 0; k < 4; k++) worker.postMessage({ n: i * 4 + k, work: 30000 });
    for (let k = 0; k < 4; k++) if ((await next()) !== "ack " + (i * 4 + k)) bad++;
  }
  console.log("burst rounds, wrong replies:", bad);
  worker.postMessage("stop");
  console.log("exit", await new Promise((resolve) => worker.once("exit", resolve)));
}

if (isMainThread) {
  main();
} else {
  parentPort.on("message", (m) => {
    if (m === "stop") return process.exit(0);
    burn(m.work);
    parentPort.postMessage("ack " + m.n);
  });
}
