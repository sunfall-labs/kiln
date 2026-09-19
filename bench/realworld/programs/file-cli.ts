// Tier B (needs node:fs): runs on node, bun, kiln (+ --node). Porffor and
// Hermes are capability-gated out -- they have no filesystem -- and report
// "unsupported", not a failure.
//
// Reads a JSON user array (argv[2], else the bundled fixture), validates each
// record, and prints per-city stats. Stdout is the checksum.
import fs from "node:fs";

const arg = process.argv[2] ?? "";
const file = arg === "" ? "bench/realworld/inputs/users.json" : arg;
const raw = fs.readFileSync(file, "utf8");
const users = JSON.parse(raw);

const counts: Record<string, number> = {};
let sum = 0;
let bad = 0;
for (const u of users) {
  if (typeof u.name !== "string" || typeof u.age !== "number" || typeof u.city !== "string") {
    bad++;
    continue;
  }
  counts[u.city] = (counts[u.city] ?? 0) + 1;
  sum += u.age;
}

for (const city of Object.keys(counts).sort()) {
  console.log(`${city} count=${counts[city]}`);
}
console.log(`total=${users.length} bad=${bad} avgAge=${(sum / (users.length - bad)).toFixed(2)}`);
