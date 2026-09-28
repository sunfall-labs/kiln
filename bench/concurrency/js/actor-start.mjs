// Native kiln only: measure the public actor API from spawn request to ready, then its first
// method reply. Each sample creates a new realm; stop happens outside both timed intervals.
import { spawnActor } from "kiln:actors";
import { arg, emit, median, now } from "./common.mjs";

const target = new URL("./actor-start-target.mjs", import.meta.url);
const count = Number(arg(0, 20));
const starts = [];
const firstCalls = [];
let checksum = 0;

for (let index = 0; index <= count; index += 1) {
  const started = now();
  const owner = await spawnActor({ module: target, export: "Ping" });
  const ready = now();
  checksum += await owner.ref.call("reply", [index]);
  const replied = now();
  starts.push(ready - started);
  firstCalls.push(replied - ready);
  await owner.stop();
}

emit({
  case: "actor-start",
  cold_ms: starts[0],
  median_ms: median(starts.slice(1)),
  first_call_ms: median(firstCalls.slice(1)),
  checksum: String(checksum),
});
