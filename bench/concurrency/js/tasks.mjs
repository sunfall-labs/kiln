// 4. Many concurrent tasks in one realm (node, bun, kiln): N async tasks, each waking `ticks`
// times on a timer of `interval` ms and doing a small piece of work. The floor is
// ticks * interval; everything above it is scheduling cost. Peak memory is measured by run.py
// from outside (the process's maximum resident set).
import { arg, emit, now } from "./common.mjs";

const n = Number(arg(0, 1000));
const ticks = Number(arg(1, 10));
const interval = Number(arg(2, 10));

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let total = 0;
async function task(id) {
  for (let t = 0; t < ticks; t += 1) {
    await sleep(interval);
    total += (id % 7) + 1;
  }
}

const started = now();
const running = [];
for (let i = 0; i < n; i += 1) running.push(task(i));
await Promise.all(running);
const wall = now() - started;
emit({ case: `tasks-${n}`, wall_ms: wall, ticks_per_s: (n * ticks) / (wall / 1000), checksum: String(total) });
