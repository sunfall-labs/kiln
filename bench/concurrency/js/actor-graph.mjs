// Compare ordinary copying, a private prepared snapshot, and a factory-built direct move.
// Direct construction creates a fresh graph each time, so report that work separately.
import { isolate, prepareTransfer, spawnActor, transfer } from "kiln:actors";
import { arg, buildGraph, emit, median, now, walkGraph } from "./common.mjs";

const target = new URL("./actor-graph-target.mjs", import.meta.url);
const samples = Number(arg(0, 10));
const graph = buildGraph();
const expected = walkGraph(graph);
const owner = await spawnActor({ module: target, export: "GraphReader" });
const copied = [];
const roundTrips = [];
const prepared = [];
const handedOff = [];
const totals = [];
const directBuilds = [];
const directHandoffs = [];
const directTotals = [];

async function copy() {
  const started = now();
  const answer = await owner.ref.call("sum", [graph]);
  const elapsed = now() - started;
  if (answer !== expected) throw new Error("copied graph checksum changed");
  return elapsed;
}

async function move() {
  const started = now();
  const handle = prepareTransfer(graph);
  const ready = now();
  const answer = await owner.ref.call("sum", [transfer(handle)]);
  const done = now();
  if (answer !== expected) throw new Error("prepared graph checksum changed");
  return [ready - started, done - ready, done - started];
}

async function roundTrip() {
  const started = now();
  const answer = await owner.ref.call("echo", [graph]);
  const elapsed = now() - started;
  if (answer === graph || walkGraph(answer) !== expected) {
    throw new Error("round-trip graph checksum or ownership changed");
  }
  return elapsed;
}

async function direct() {
  const started = now();
  const handle = isolate(buildGraph);
  const ready = now();
  const answer = await owner.ref.call("sum", [transfer(handle)]);
  const done = now();
  if (answer !== expected) throw new Error("direct graph checksum changed");
  return [ready - started, done - ready, done - started];
}

// Bring up the actor and both paths before sampling. Alternate their order across pairs so
// time-varying host load does not consistently favor one path.
await copy();
await move();
await roundTrip();
await direct();
for (let index = 0; index < samples; index += 1) {
  if (index % 2 === 0) {
    copied.push(await copy());
    roundTrips.push(await roundTrip());
    const [prep, handoff, total] = await move();
    prepared.push(prep);
    handedOff.push(handoff);
    totals.push(total);
    const [build, directHandoff, directTotal] = await direct();
    directBuilds.push(build);
    directHandoffs.push(directHandoff);
    directTotals.push(directTotal);
  } else {
    const [build, directHandoff, directTotal] = await direct();
    directBuilds.push(build);
    directHandoffs.push(directHandoff);
    directTotals.push(directTotal);
    const [prep, handoff, total] = await move();
    prepared.push(prep);
    handedOff.push(handoff);
    totals.push(total);
    roundTrips.push(await roundTrip());
    copied.push(await copy());
  }
}
await owner.stop();

const checksum = String(expected);
emit({ case: "actor-graph-copy", ms_per_op: median(copied), checksum });
emit({ case: "actor-graph-roundtrip-copy", ms_per_op: median(roundTrips), checksum });
emit({ case: "actor-graph-prepare", ms_per_op: median(prepared), checksum });
emit({ case: "actor-graph-handoff", ms_per_op: median(handedOff), checksum });
emit({ case: "actor-graph-prepared-total", ms_per_op: median(totals), checksum });
emit({ case: "actor-graph-direct-build", ms_per_op: median(directBuilds), checksum });
emit({ case: "actor-graph-direct-handoff", ms_per_op: median(directHandoffs), checksum });
emit({ case: "actor-graph-direct-total", ms_per_op: median(directTotals), checksum });
