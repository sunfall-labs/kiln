// An array insert that grows the array must publish the new `count` before it fills the gap.
//
// The collector traces an array's first `count` elements and no further, and converting an
// argument to a stored value allocates for any string of more than one byte -- so a value written
// past `count` is invisible to the collector, and the next conversion in the same loop collects
// the string just stored and hands its memory to something else. The wrong answers were
// `2,s1,s8,s8,...` and, once the freed memory had been reused, the array's own new length
// arriving as an element.
//
// This only bites when a collection lands inside the loop, so it passes without
// `KILN_GC_STRESS` and the plain corpus run cannot see it. `mise run native-stress` is what
// catches this class; see `.scratch/array-splice-gc/issues/01`.

// splice: insert wider than the removal, so the array has to grow.
const a = [2, 4];
a.splice(1, 0, "s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8");
console.log(a.length + ":" + a.join(","));

// Read back by index as well as by join, so a corrupt element cannot hide behind the join.
let byIndex = "";
for (let i = 0; i < a.length; i++) byIndex += i + "=" + a[i] + " ";
console.log(byIndex.trim());

// unshift: the same gap, opened at the front.
const b = [2, 4];
b.unshift("u1", "u2", "u3", "u4", "u5", "u6", "u7", "u8");
console.log(b.length + ":" + b.join(","));

// splice with a removal as well as an insertion, so the tail moves too.
const c = [1, 2, 3, 4, 5];
c.splice(1, 2, "r1", "r2", "r3", "r4", "r5", "r6");
console.log(c.length + ":" + c.join(","));

// Several sizes, since the number of elements written before the first collection varies.
for (const n of [1, 2, 3, 5, 9, 17]) {
  const d = [0, 99];
  const args = [];
  for (let i = 0; i < n; i++) args.push("x" + i + "y");
  d.splice(1, 0, ...args);
  console.log(n + ":" + d.join(","));
}

// The values must be the strings themselves, not merely the right count of them.
const e = [];
e.unshift("aa", "bb", "cc", "dd");
console.log(e.every((s, i) => s === ["aa", "bb", "cc", "dd"][i]));

// Capture groups land in a fresh match array the same way.
const m = /(pp)(qq)(rr)(ss)/.exec("ppqqrrss");
console.log(m.length + ":" + m.slice(1).join(","));
