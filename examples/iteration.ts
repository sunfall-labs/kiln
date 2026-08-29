// `for-of` goes through the iterator protocol rather than indexing: the subject may be any
// iterable, and indexing would be wrong for everything that is not an array.
const doubled: number[] = [];
for (const value of [1, 2, 3]) {
  doubled.push(value * 2);
}
console.log("for-of", doubled.join(","));

for (const value of [1, 2, 3, 4]) {
  if (value === 2) {
    continue;
  }
  if (value === 4) {
    break;
  }
  console.log("continue and break", value);
}

// An iterator is itself iterable, which is what lets one be consumed by a second loop.
const iterator = [10, 20].values();
const drained: number[] = [];
for (const value of iterator) {
  drained.push(value);
}
console.log("iterator is iterable", drained.join(","));

// `[][Symbol.iterator]` and `[].values` are the same function, and the loop reaches it by
// symbol rather than by name.
console.log("same function", [][Symbol.iterator] === [].values);

// The loop binding is fresh each iteration, so a closure captures that step's value rather
// than the last one. This is the difference between `let` and `var` semantics, and it is the
// reason the binding is declared inside the loop body's scope.
const closures: Array<() => number> = [];
for (const n of [1, 2, 3]) {
  closures.push(() => n);
}
console.log("per-iteration binding", closures[0](), closures[1](), closures[2]());
