// kiln-native: profile=native-release parallelism=1,2,max
//
// Tier 4 (.scratch/parallel-builtins issue 09), what stays sequential: a call to a built-in that
// is not marked pure (`Math.random`, `console.log`, `push`), to a function that writes, to one
// that is reassigned, anywhere in the call chain -- `--explain-parallel` names each (checked in
// `parallel_tests.rs`) -- and every effect happens exactly as `map`'s, in order. A patched
// built-in stands every parallel site down through the realm's modified-built-ins record.
const n = typeof process !== "undefined" && process.env.KILN_GC_STRESS ? 1501 : 20001;
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = JSON.stringify(p[i]) === JSON.stringify(m[i]);
  console.log(label, p.length, JSON.stringify(p[4]), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallel differs from sequential: " + label);
}
const lines = [];
for (let i = 0; i < n; i++) lines.push(" w" + i + " ");
let calls = 0;
function bump(s) { calls += 1; return s.trim(); }
function outer(s) { return inner(s) + "!"; }
function inner(s) { return bump(s); }
const seen = [];
function note(s) { if (s === " w7 ") seen.push(s); return s.length; }
let swap = s => s.trim();
swap = s => s.toUpperCase();

check("a helper that writes", lines.parallelMap(bump), lines.map(bump));
console.log("calls", calls);
check("the write two calls down", lines.parallelMap(outer), lines.map(outer));
console.log("calls", calls);
check("push", lines.parallelMap(l => { const a = [l]; a.push(1); return a.length; }), lines.map(l => { const a = [l]; a.push(1); return a.length; }));
check("a reassigned helper", lines.parallelMap(l => swap(l)), lines.map(l => swap(l)));
check("a write to a captured array", lines.parallelMap(note), lines.map(note));
console.log("seen", seen.length);
check("random stays sequential", lines.parallelMap(l => Math.random() < 2 ? l.length : 0), lines.map(l => Math.random() < 2 ? l.length : 0));
String.prototype.padEnd = function (k) { calls += 1; return "P" + this + k; };
const padded = lines.parallelMap(l => l.trim().padEnd(9));
const sequential = lines.map(l => l.trim().padEnd(9));
console.log("patched", JSON.stringify(padded) === JSON.stringify(sequential));
