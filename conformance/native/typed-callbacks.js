// Typed callback fast paths for `map`/`filter`/`reduce`: a callback whose proto this compile
// proves statically -- an inline arrow, or a binding that only ever holds one closure -- over a
// numeric array, is entered through its own typed C entry directly from the intrinsic loop
// instead of through the boxed call protocol. This corpus exercises the split between that path
// and the generic (boxed) one it still falls back to, and the mutation edge cases the typed
// loop has to answer the same way as the generic one and the interpreter.

// 1. Numeric arrays, arrow callbacks of one and two declared parameters.
const nums = [1, 2, 3, 4, 5];
console.log("map-1", nums.map(x => x * 2 + 1));
console.log("map-2", nums.map((x, i) => x * 2 + i));
console.log("filter-1", nums.filter(x => x % 2 === 0));
console.log("filter-2", nums.filter((x, i) => i % 2 === 0));
console.log("reduce", nums.reduce((acc, x) => acc + x, 0));

// 2. The same shapes over an array holding a string: not numeric, so the generic path runs.
const mixed = [1, 2, "three", 4];
console.log("map-mixed", mixed.map(x => (typeof x === "number" ? x * 2 : x)));
console.log("filter-mixed", mixed.filter(x => typeof x === "number"));
console.log(
  "reduce-mixed",
  mixed.reduce((acc, x) => (typeof x === "number" ? acc + x : acc + x.length), 0),
);

// 3. A sparse array: a hole clears `is_numeric`, so this is always the generic path -- map
// preserves the hole, filter and reduce skip it without calling the callback.
const sparse = [1, , 3, , 5];
console.log("map-sparse", sparse.map(x => x * 10));
console.log("filter-sparse", sparse.filter(x => x > 2));
console.log("reduce-sparse", sparse.reduce((acc, x) => acc + x, 0));

// 4. A callback that throws partway through: the partial state left behind must match node's.
const throwSrc = [1, 2, 3, 4, 5];
try {
  throwSrc.map(x => {
    if (x === 3) throw new Error("boom-map");
    return x * 2;
  });
} catch (e) {
  console.log("map-throw", e.message);
}
const filterSeenBeforeThrow = [];
try {
  throwSrc.filter(x => {
    if (x === 3) throw new Error("boom-filter");
    filterSeenBeforeThrow.push(x);
    return true;
  });
} catch (e) {
  console.log("filter-throw", e.message, JSON.stringify(filterSeenBeforeThrow));
}
try {
  throwSrc.reduce((acc, x) => {
    if (x === 3) throw new Error("boom-reduce");
    return acc + x;
  }, 0);
} catch (e) {
  console.log("reduce-throw", e.message);
}

// 5. A callback that mutates the array while iterating: push (which grows what map/filter would
// see if they re-read the length, and does grow what this engine's own dense `reduce` sees,
// matching the interpreter's dense fast path rather than the specification -- see
// direct.rs / call_method.rs for why), and assignment to a later, not-yet-visited index (which
// changes nothing about which indices are visited, only what is read at one of them).
function mapPush() {
  const arr = [1, 2, 3];
  const out = arr.map(x => {
    if (arr.length < 6) arr.push(x);
    return x * 2;
  });
  return JSON.stringify(out) + " " + JSON.stringify(arr);
}
console.log("map-push", mapPush());

function filterPush() {
  const arr = [1, 2, 3];
  const out = arr.filter(x => {
    if (arr.length < 6) arr.push(x);
    return x > 1;
  });
  return JSON.stringify(out) + " " + JSON.stringify(arr);
}
console.log("filter-push", filterPush());

function mapAssignLater() {
  const arr = [1, 2, 3, 4];
  const out = arr.map((x, i) => {
    if (i === 0) arr[3] = 100;
    return x;
  });
  return JSON.stringify(out);
}
console.log("map-assign-later", mapAssignLater());

function filterAssignLater() {
  const arr = [1, 2, 3, 4];
  const out = arr.filter((x, i) => {
    if (i === 0) arr[3] = 100;
    return x > 1;
  });
  return JSON.stringify(out);
}
console.log("filter-assign-later", filterAssignLater());

function reduceAssignLater() {
  const arr = [1, 2, 3, 4];
  return arr.reduce((acc, x, i) => {
    if (i === 0) arr[3] = 100;
    return acc + x;
  }, 0);
}
console.log("reduce-assign-later", reduceAssignLater());

// 6. A callback that returns a non-number sometimes: its typed entry cannot prove it returns a
// number, so the typed loop calls it through the `kiln_val`-returning entry, not the
// `double`-returning one.
console.log("map-sometimes-string", nums.map(x => (x > 2 ? x : "s")));

// 7. `reduce` with a non-number initial value: the accumulator can never be a double, so this
// stays on the generic path even though the array is numeric and the callback's proto is known.
console.log("reduce-string-seed", nums.reduce((acc, x) => acc + x, ""));

// 8. `this` in a named, non-arrow, *strict* callback: `this` stays exactly what it was called
// with (`undefined`, since `map` supplies no receiver), never substituted. Kiln compiles this
// whole file as a module -- strict by definition, per `native.sh`'s own note on why sloppy code
// needs a `.cjs` entry -- but node runs a bare `.js` as CommonJS (sloppy), so the directive is
// required here, not decorative: without it this callback would be sloppy under node and
// strict under kiln, and the two engines would disagree for a reason that has nothing to do
// with this corpus. Passed inline, not through a hoisted binding: `plan.known` (see direct.rs)
// only proves identity through a `MakeFunction` value or a read through a cell, an upvalue, or
// a global -- not a plain local -- so a named declaration referenced after the `try`/`catch`
// blocks above (a second basic block) would take the generic path instead, answering the same
// thing but not exercising the typed loop's own receiver computation, which is the point here.
// The sloppy half (global-object substitution) is `typed-callbacks-sloppy.cjs`, beside this
// file, which is the only way a genuinely sloppy proto reaches this corpus.
console.log(
  "map-this-strict",
  nums.map(function strictThis(x) {
    "use strict";
    return this === undefined ? "undefined" : String(this);
  }),
);

// 9. An arrow capturing `this` inside a class method: the callback is an arrow, so it is called
// with its captured `this`, not the array.
class Doubler {
  constructor(factor) {
    this.factor = factor;
  }
  scaleAll(arr) {
    return arr.map(x => x * this.factor);
  }
}
console.log("map-class-this", new Doubler(3).scaleAll(nums));

// 10. Nested map inside map.
console.log(
  "map-nested",
  [[1, 2], [3, 4]].map(row => row.map(x => x * 10)),
);

// 11. The benchmark's own pattern, with its checksum.
function benchmarkPattern() {
  const values = [];
  for (let i = 0; i < 5000; i++) values.push(i);
  let checksum = 0;
  for (let iter = 0; iter < 100; iter++) {
    const result = values
      .map(x => x * 2 + 1)
      .filter(x => x % 3 === 0)
      .reduce((acc, x) => acc + x, 0);
    checksum += result;
  }
  return checksum;
}
console.log("benchmark-checksum", benchmarkPattern());
