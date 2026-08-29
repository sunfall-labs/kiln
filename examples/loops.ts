// `for` is its own construct rather than sugar over `while`: `continue` must run the update
// clause, and a `let` in the initialiser belongs to the loop rather than the enclosing block.
let sum = 0;
for (let i = 0; i < 5; i++) {
  sum += i;
}
console.log("sum", sum);

const kept: number[] = [];
for (let i = 0; i < 6; i++) {
  if (i === 1) {
    continue;
  }
  if (i === 4) {
    break;
  }
  kept.push(i);
}
console.log("continue skips, break stops", kept.join(","));

// Nested loops: each `break`/`continue` binds to its own loop.
for (let outer = 0; outer < 2; outer++) {
  for (let inner = 0; inner < 3; inner++) {
    if (inner === 1) {
      continue;
    }
    if (inner === 2) {
      break;
    }
    console.log("pair", outer, inner);
  }
}

// Postfix yields the old value, prefix the new. That difference is why an update expression is
// not lowered to `x = x + 1`.
let n = 5;
console.log("postfix", n++, "then", n);
console.log("prefix", ++n, "then", n);
console.log("array ops", kept.indexOf(2), kept.includes(9), kept.pop(), kept.join(","));
