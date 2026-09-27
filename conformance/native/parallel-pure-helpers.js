// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 4 (.scratch/parallel-builtins issue 09), the program's own functions: a callback may call
// a function whose body satisfies the same rules -- helpers calling helpers, a helper that reads
// a constant it captures, a closure written in the callback and only called, a recursive function
// and a mutually recursive pair (every function of the cycle proven) -- and run on the pool with
// `map`'s answer. A recursion deeper than a pool thread follows declines and runs `map`.
const n = typeof process !== "undefined" && process.env.KILN_GC_STRESS ? 1501 : 20001;
function same(a, b) {
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!same(a[i], b[i])) return false;
    return true;
  }
  return Object.is(a, b);
}
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = same(p[i], m[i]);
  console.log(label, p.length, JSON.stringify(p[4]), JSON.stringify(p[p.length - 1]), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}
const RATE = 1.25;
const units = { kg: 1000, g: 1 };
function grams(amount, unit) { return amount * (units[unit] || 0); }
function price(line) {
  const parts = line.split(" ");
  return Math.round(grams(Number(parts[0]), parts[1]) * RATE) / 100;
}
function fact(k) { return k <= 1 ? 1 : k * fact(k - 1); }
function isEven(k) { return k === 0 ? true : isOdd(k - 1); }
function isOdd(k) { return k === 0 ? false : isEven(k - 1); }
function fib(k) { return k < 2 ? k : fib(k - 1) + fib(k - 2); }
function depth(k) { return k === 0 ? 0 : 1 + depth(k - 1); }
const fmt = x => x.toFixed(1);
const lines = [];
for (let i = 0; i < n; i++) lines.push((i % 50) + (i % 2 ? " kg" : " g"));

check("helpers calling helpers", lines.parallelMap(price), lines.map(price));
check("helper via an arrow", lines.parallelMap(l => fmt(price(l))), lines.map(l => fmt(price(l))));
const recur = (l, i) => [fact(i % 15), isEven(i % 40), fib(i % 12)];
check("recursion", lines.parallelMap(recur), lines.map(recur));
const written = l => { const sq = x => x * x; return sq(l.length) + [1, 2].map(y => y * 3).join(); };
check("a closure written in the callback", lines.parallelMap(written), lines.map(written));
// Deeper than the pool's call depth near the end: declines there, `map` answers.
const deep = (l, i) => depth(i === n - 3 ? 400 : i % 20);
check("a deep recursion", lines.parallelMap(deep), lines.map(deep));
const hot = l => price(l) > 30;
check("filter through a helper", lines.parallelFilter(hot), lines.filter(hot));
