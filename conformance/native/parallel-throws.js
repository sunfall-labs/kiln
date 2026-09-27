// The parallel/sequential differential harness (.scratch/parallel-builtins issue 02): exceptions.
// When several elements throw, `map` throws the first one it reaches -- the lowest index -- and
// never calls the callback again; `parallelMap` must throw the same exception after the same
// calls. A callback that can throw does not qualify for the pool today, so these all take the
// sequential lowering, and are here so that stays true as the tiers widen.

function describe(f, log) {
  try {
    const r = f();
    return "returned " + r.length + " after " + log.length + " calls";
  } catch (e) {
    // The message only for the callback's own errors: the engines word the language's
    // `TypeError`s differently, which is not what this probe compares.
    const what = e instanceof Error ? e.name + (e.name === "TypeError" ? "" : ": " + e.message) : "value " + String(e);
    return "threw " + what + " after " + log.length + " calls, last " + log[log.length - 1];
  }
}
function check(label, p, m) {
  console.log(label, p, p === m ? "same" : "DIFFERENT " + m);
  if (p !== m) throw new Error("parallelMap differs from map: " + label);
}

const xs = [];
for (let i = 0; i < 3000; i++) xs.push(i);

{
  const lp = [];
  const lm = [];
  const p = describe(() => xs.parallelMap((x, i) => { lp.push(i); if (i === 3 || i === 7 || i === 2999) throw new Error("at " + i); return x; }), lp);
  const m = describe(() => xs.map((x, i) => { lm.push(i); if (i === 3 || i === 7 || i === 2999) throw new Error("at " + i); return x; }), lm);
  check("lowest-index", p, m);
}
{
  const lp = [];
  const lm = [];
  const p = describe(() => xs.parallelMap((x, i) => { lp.push(i); if (i >= 1800) throw new RangeError("late " + i); return x; }), lp);
  const m = describe(() => xs.map((x, i) => { lm.push(i); if (i >= 1800) throw new RangeError("late " + i); return x; }), lm);
  check("late-chunk", p, m);
}
{
  const lp = [];
  const lm = [];
  const p = describe(() => xs.parallelMap((x, i) => { lp.push(i); if (i % 500 === 499) throw i; return x; }), lp);
  const m = describe(() => xs.map((x, i) => { lm.push(i); if (i % 500 === 499) throw i; return x; }), lm);
  check("thrown-value", p, m);
}
{
  // A TypeError the language throws, not the callback: a BigInt mixed with a number.
  const lp = [];
  const lm = [];
  const p = describe(() => xs.parallelMap((x, i) => { lp.push(i); return i === 5 ? x * 2n : x; }), lp);
  const m = describe(() => xs.map((x, i) => { lm.push(i); return i === 5 ? x * 2n : x; }), lm);
  check("bigint-mix", p, m);
}
{
  // Not callable: both throw before calling anything.
  const p = describe(() => xs.parallelMap(42), []);
  const m = describe(() => xs.map(42), []);
  check("not-callable", p, m);
}
{
  // Throwing on a typed array.
  const ta = new Float64Array(3000);
  for (let i = 0; i < ta.length; i++) ta[i] = i;
  const lp = [];
  const lm = [];
  const p = describe(() => ta.parallelMap((x, i) => { lp.push(i); if (x === 17 || x === 18) throw new Error("typed " + x); return x; }), lp);
  const m = describe(() => ta.map((x, i) => { lm.push(i); if (x === 17 || x === 18) throw new Error("typed " + x); return x; }), lm);
  check("typed", p, m);
}
