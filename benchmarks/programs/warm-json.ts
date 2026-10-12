const rows: { id: number; label: string; value: number }[] = [];
for (let i = 0; i < 250; i++) rows.push({ id: i, label: `item-${i}`, value: i * 3 });
const input = JSON.stringify(rows);
function workload(seed: number): number {
  const parsed = JSON.parse(input);
  let total = seed;
  for (const row of parsed) total += row.value;
  return (total + JSON.stringify(parsed).length) >>> 0;
}

// Warm first, then calibrate a multiple-of-16 batch to >= 100 ms.
// Every result contributes to a checksum verified against Node.
for (let i = 0; i < 200; i++) workload(i % 16);
let iterations = 16;
let elapsedMs = 0;
let checksum = 0;
do {
  checksum = 0;
  const start = Date.now();
  for (let i = 0; i < iterations; i++) checksum = (checksum + workload(i % 16)) >>> 0;
  elapsedMs = Date.now() - start;
  if (elapsedMs < 100 && iterations < 131072) iterations *= 2;
  else break;
} while (elapsedMs < 100 && iterations <= 131072);
const checks: number[] = [];
for (let i = 0; i < 16; i++) checks.push(workload(i));
console.log(JSON.stringify({ elapsedMs, iterations, checksum, checks }));
