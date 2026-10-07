
// Heavy callback timing (issue 14).
const N = 200000;
const hs = [];
for (let i = 0; i < N; i++) hs.push((i % 1000) * 0.001);
const t0 = Date.now();
const out = hs.parallelMap(x => { let s = x; for (let k = 0; k < 2000; k++) s = s * 0.999 + Math.sqrt(k + x); return s; });
const t1 = Date.now();
let sum = 0;
for (let i = 0; i < out.length; i += 997) sum += out[i];
console.log("heavy", out.length, sum.toFixed(6));
console.error("heavy-ms", t1 - t0);
