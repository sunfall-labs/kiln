// kiln-native: profile=native-release parallelism=1,2,max
//
// The parallel family where the pool may not run (.scratch/parallel-builtins issue 10): a
// callback with a side effect, one that throws, one that reads the heap, a capture that is not a
// number, a receiver that is not a numeric array. Each runs the sequential definition, so the
// calls, their order, the exception and the answer are exactly the sequential method's.
const n = 20000;
const xs = [];
for (let i = 0; i < n; i++) xs.push((i * 31) % 97);

let seen = [];
const f1 = xs.parallelFilter((x, i) => { if (i % 5000 === 0) seen.push(i); return x > 50; });
console.log("filter effects", f1.length, seen.join());
seen = [];
const r1 = xs.parallelReduce((a, x, i) => { if (i % 4096 === 0) seen.push(i); return a + x; }, (a, b) => { seen.push(-1); return a + b; }, 0);
console.log("reduce effects", r1, seen.length, seen.slice(0, 6).join());

function attempt(label, f) {
  let calls = 0;
  try {
    console.log(label, "returned", String(f(() => ++calls)));
  } catch (e) {
    // The message of a TypeError the engine raises is its own; the type and the call count
    // are the method's.
    const message = e instanceof RangeError ? e.message : e instanceof TypeError ? "TypeError" : String(e);
    console.log(label, "threw", message, "after", calls);
  }
}
attempt("filter throws", tick => xs.parallelFilter(x => { tick(); if (x === 96) throw new RangeError("at " + x); return true; }).length);
attempt("reduce throws", tick => xs.parallelReduce((a, x, i) => { tick(); if (i === 12345) throw new RangeError("at " + i); return a + x; }, (a, b) => a + b, 0));
attempt("combine throws", tick => xs.parallelReduce((a, x) => a + x, (a, b) => { tick(); if (b > 0) throw new RangeError("combine"); return a + b; }, 0));
attempt("sort throws", tick => xs.slice().parallelSort((a, b) => { tick(); if (a === 13 && b === 14) throw new RangeError("cmp"); return a - b; }).length);
attempt("reduce not callable", () => xs.parallelReduce((a, x) => a + x, 5, 0));
attempt("filter not callable", () => xs.parallelFilter(null));
attempt("sort bad comparator", () => xs.parallelSort(1));
attempt("null receiver", () => Array.prototype.parallelReduce.call(null, (a, x) => a + x, (a, b) => a + b, 0));
attempt("typed receiver check", () => Object.getPrototypeOf(Uint8Array.prototype).parallelFilter.call([1, 2], x => x));

const rows = xs.map(x => ({ v: x }));
console.log("heap read", rows.parallelFilter(r => r.v > 90).length, rows.parallelReduce((a, r) => a + r.v, (a, b) => a + b, 0));
const word = "k";
console.log("string capture", xs.parallelFilter(x => word + x === "k5").length);
let bump = 1;
bump = 2;
console.log("reassigned capture", xs.parallelFilter(x => x * bump > 150).length, xs.parallelReduce((a, x) => a + x * bump, (a, b) => a + b, 0));
const mixed = xs.slice(0, 1000);
mixed[3] = "7";
console.log("mixed receiver", mixed.parallelFilter(x => x > 50).length, mixed.parallelReduce((a, x) => a + x, (a, b) => a + b, 0).length);
const tagged = { limit: 60 };
console.log("thisArg", xs.parallelFilter(function (x) { return x > this.limit; }, tagged).length);
const like = { length: 3, 0: 5, 1: 1, 2: 4 };
console.log("array-like", Array.prototype.parallelFilter.call(like, x => x > 2).join(), Array.prototype.parallelReduce.call(like, (a, x) => a + x, (a, b) => a + b, 0));
console.log(Array.prototype.parallelFilter.name, Array.prototype.parallelFilter.length, Array.prototype.parallelReduce.length, Array.prototype.parallelSort.length);
// ADR 0068 rule 2: never enumerable.
const walked = [];
for (const key in [7]) walked.push(key);
for (const key in new Float64Array(1)) walked.push(key);
console.log(walked.join(), JSON.stringify(Object.getOwnPropertyDescriptor(Array.prototype, "parallelReduce").enumerable));
