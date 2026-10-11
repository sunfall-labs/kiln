import { bench } from "./harness.mjs";

const ROUNDS = 8192;

function joinKey(prefix: string, suffix: string): string {
  return prefix + suffix;
}

// This literal forces the canonical spelling into the program's shared-key inventory. The key
// passed to the write loop is assembled dynamically, so the computed-write path still supplies a
// non-symbol key and calls intern_key on every iteration.
const sharedSeed: Record<string, number> = { "cache-A12345Z": 1 };
const sharedKey = joinKey("cache-A12345", "Z");
const sharedTarget: Record<string, number> = {};
sharedTarget[sharedKey] = 0;

// Same length and first/last UTF-16 units as cache-A12345Z, but a different middle character.
// This key is assembled dynamically and is not itself a source literal, so it should stay
// realm-local. It collides with the shared cache signature and exercises full equality fallback.
const localCollisionKey = joinKey("cache-B12345", "Z");
const localCollisionTarget: Record<string, number> = {};
localCollisionTarget[localCollisionKey] = 0;

// A separate realm-local spelling with a distinct signature checks the fast negative hint path
// before the ordinary shared miss and realm-local key-table hit.
const localDistinctKey = joinKey("realm-local-unique-", "tail");
const localDistinctTarget: Record<string, number> = {};
localDistinctTarget[localDistinctKey] = 0;

function repeatedComputedWrite(target: Record<string, number>, key: string): number {
  let sum = 0;
  for (let i = 0; i < ROUNDS; i += 1) {
    target[key] = i;
    sum += i;
  }
  return sum + target[key];
}

function repeatedOwnSlotRead(target: Record<string, number>, key: string): number {
  let sum = 0;
  for (let i = 0; i < ROUNDS; i += 1) sum += target[key];
  return sum;
}

// Validate the seed outside timing. This also makes the canonical shared key live in the same
// realm before any benchmark row starts.
if (sharedSeed["cache-A12345Z"] !== 1) throw new Error("shared-key seed mismatch");

bench("shared-canonical-dynamic-write", () =>
  repeatedComputedWrite(sharedTarget, sharedKey));
bench("realm-local-signature-collision-write", () =>
  repeatedComputedWrite(localCollisionTarget, localCollisionKey));
bench("realm-local-distinct-signature-write", () =>
  repeatedComputedWrite(localDistinctTarget, localDistinctKey));
// Regression-only control: the warmed read PIC returns before intern_key, so this row must not be
// cited as evidence for the shared-key cache's benefit.
bench("warmed-own-slot-read-regression-control", () =>
  repeatedOwnSlotRead(sharedTarget, sharedKey));
