// What arming a timer costs: `setTimeout`, `setInterval` and `setImmediate` each hand back a
// handle object (`Timeout`/`Immediate`), so every call allocates one, and clearing one reads its
// identity back from it. Nothing fires inside the timed function -- the timers are cleared
// before the loop runs -- so this is the arm and clear path alone; `bench/concurrency/js/tasks.mjs`
// is the one that also runs a million of them.
import { bench } from "../harness.mjs";

const noop = () => {};

bench("timer-create", () => {
  let count = 0;
  for (let i = 0; i < 5000; i++) {
    const timeout = setTimeout(noop, 100000);
    const interval = setInterval(noop, 100000);
    const immediate = setImmediate(noop);
    clearTimeout(timeout);
    clearInterval(interval);
    clearImmediate(immediate);
    count += 3;
  }
  return count;
}, { externalIterations: 200 });
