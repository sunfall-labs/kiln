// Hold the same ordinary or frozen object graph in N public actors. Each mode runs in a fresh
// process so allocator pages from the other mode cannot affect the held-RSS measurement.
import { spawnActor } from "kiln:actors";
import { share } from "kiln:shared";
import { arg, buildGraph, emit, now, rssKiB, walkGraph } from "./common.mjs";

const mode = arg(0, "copy");
const count = Number(arg(1, 4));
if ((mode !== "copy" && mode !== "shared") || !Number.isSafeInteger(count) || count < 1) {
  throw new Error("expected copy|shared and a positive actor count");
}

const target = new URL("./actor-shared-target.mjs", import.meta.url);
const graph = buildGraph();
const expected = walkGraph(graph);
const owners = [];
let prepareMs = 0;
let setupMs = 0;
let queryMs = 0;
let beforeRssKiB = -1;
let heldRssKiB = -1;
let checksum = 0;
try {
  for (let index = 0; index < count; index += 1) {
    owners.push(await spawnActor({ module: target, export: "SharedGraphReader" }));
  }
  beforeRssKiB = rssKiB();

  let started = now();
  const dataset = mode === "shared" ? share(graph) : graph;
  prepareMs = now() - started;

  started = now();
  for (const owner of owners) {
    const answer = await owner.ref.call("load", [dataset]);
    if (answer.sum !== expected || answer.shared !== (mode === "shared")) {
      throw new Error(`actor dataset or shared identity changed: ${answer.sum}/${expected}, ${answer.shared}/${mode}`);
    }
  }
  setupMs = now() - started;
  heldRssKiB = rssKiB();

  started = now();
  const answers = await Promise.all(owners.map((owner) => owner.ref.call("query", [])));
  queryMs = now() - started;
  for (const answer of answers) {
    if (answer !== expected) throw new Error("actor retained graph changed");
    checksum += answer;
  }
} finally {
  await Promise.all(owners.map((owner) => owner.stop()));
}

emit({ case: `actor-shared-${count}-${mode}`, prepare_ms: prepareMs, setup_ms: setupMs,
  query_ms: queryMs, before_rss_kib: beforeRssKiB, held_rss_kib: heldRssKiB,
  dataset_rss_kib: heldRssKiB - beforeRssKiB, checksum: String(checksum) });
