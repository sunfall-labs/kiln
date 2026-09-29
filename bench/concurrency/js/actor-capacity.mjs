// Public actor capacity: keep N independent actor realms alive, call each once, then stop them.
// A bounded request window avoids measuring a pile of pending spawn promises as realm memory.
import fs from "node:fs";
import { spawnActor } from "kiln:actors";
import { arg, emit, now, rssKiB } from "./common.mjs";

const target = new URL("./actor-start-target.mjs", import.meta.url);
const count = Number(arg(0, 1000));
const window = 64;
if (!Number.isSafeInteger(count) || count < 1) throw new Error("actor count must be positive");

function threads() {
  try {
    const status = fs.readFileSync("/proc/self/status", "utf8");
    const match = /Threads:\s+(\d+)/.exec(status);
    return match ? Number(match[1]) : -1;
  } catch {
    return -1;
  }
}

// Warm the linked module and actor code before timing a capacity run.
const warm = await spawnActor({ module: target, export: "Ping" });
if (await warm.ref.call("reply", [0]) !== 1) throw new Error("warm actor replied incorrectly");
await warm.stop();

const owners = [];
let checksum = 0;
let spawnMs = 0;
let callMs = 0;
let stopMs = 0;
let heldRssKiB = -1;
let heldThreads = -1;
try {
  let started = now();
  for (let base = 0; base < count; base += window) {
    const settled = await Promise.allSettled(Array.from(
      { length: Math.min(window, count - base) },
      () => spawnActor({ module: target, export: "Ping" }),
    ));
    for (const result of settled) if (result.status === "fulfilled") owners.push(result.value);
    const rejected = settled.find((result) => result.status === "rejected");
    if (rejected) throw rejected.reason;
  }
  spawnMs = now() - started;
  heldRssKiB = rssKiB();
  heldThreads = threads();

  started = now();
  for (let base = 0; base < count; base += window) {
    const replies = await Promise.all(owners.slice(base, base + window)
      .map((owner, offset) => owner.ref.call("reply", [base + offset])));
    for (let index = 0; index < replies.length; index += 1) {
      const expected = base + index + 1;
      if (replies[index] !== expected) throw new Error(`actor ${base + index} replied incorrectly`);
      checksum += replies[index];
    }
  }
  callMs = now() - started;
} finally {
  const started = now();
  for (let base = 0; base < owners.length; base += window) {
    await Promise.all(owners.slice(base, base + window).map((owner) => owner.stop()));
  }
  stopMs = now() - started;
}

if (owners.length !== count || checksum !== count * (count + 1) / 2) {
  throw new Error("actor capacity count or checksum changed");
}
emit({ case: `actor-capacity-${count}`, spawn_ms: spawnMs, calls_ms: callMs,
  stop_ms: stopMs, held_rss_kib: heldRssKiB, held_threads: heldThreads,
  checksum: String(checksum) });
