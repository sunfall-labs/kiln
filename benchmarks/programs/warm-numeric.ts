function workload(seed: number): number {
  let value = seed + 1;
  for (let i = 0; i < 100000; i++) value = (Math.imul(value, 1664525) + 1013904223) >>> 0;
  return value;
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
