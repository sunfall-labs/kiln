import { bench } from "../../bench/harness.mjs";

const lookupKeys = Array.from({ length: 256 }, (_, index) =>
  `prototype-probe-${String(index).padStart(4, "0")}-${"x".repeat(96)}`,
);
const parents = Array.from({ length: 4 }, () => Object.create(null));
for (let index = 0; index < lookupKeys.length; index += 1) {
  parents[0][lookupKeys[index]] = index + 1;
}

function makeDictionary(level) {
  const object = Object.create(parents[level]);
  for (let index = 0; index < 48; index += 1) {
    object[`filler-${level}-${String(index).padStart(3, "0")}`] = index;
  }
  object[`discard-${level}`] = -1;
  delete object[`discard-${level}`];
  return object;
}
for (let level = 1; level < parents.length; level += 1) {
  parents[level] = makeDictionary(level - 1);
}
const receiver = makeDictionary(parents.length - 1);
const iterations = 65_536;
const expectedChecksum = 8_421_376;

// Prime every dynamic spelling before entering the harness's timed region.
for (const key of lookupKeys) receiver[key];

function workload() {
  let checksum = 0;
  for (let index = 0; index < iterations; index += 1) {
    const key = lookupKeys[(index * 73 + 19) & 255];
    checksum = (checksum + receiver[key]) | 0;
  }
  return checksum;
}

// Warm the workload function and its property-read site outside timed samples.
if (workload() !== expectedChecksum) throw new Error("dictionary workload checksum mismatch");

// Refuse virtual and deterministic step-per-read clocks before timing. The harness repeats
// this classification when it runs; keeping the preflight here makes external mode impossible
// to mistake for an in-process benchmark in the captured output.
const reads = Array.from({ length: 5 }, () => Date.now());
if (reads.slice(1).every((value, index) => value - reads[index] === 1)) {
  throw new Error("in-process real clock required; detected stepping clock");
}
let clockSink = 0;
const clockStarted = Date.now();
for (let index = 0; index < 3_000_000; index += 1) clockSink += index;
if (Date.now() - clockStarted <= 0 || clockSink === 0) {
  throw new Error("in-process real clock required; detected frozen clock");
}

bench("dictionary-key-hash", workload, { budgetMs: 2000 });
