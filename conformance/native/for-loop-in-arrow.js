// A `for` loop whose induction variable lives in an arrow's locals. On 2026-09-05 the SSA value
// graph emitter (ADR 0056) compiled this to an infinite loop: it reused the SSA value it knew for
// `i` before the loop instead of merging the back edge with a phi, so the header compared `0 < 5`
// forever. The plain-function form did not reach the graph emitter and hid the bug.
const run = () => {
  let sum = 0;
  for (let i = 0; i < 5; i++) sum += i % 7;
  return sum;
};
console.log(run());

function plain(n) {
  let i = 0;
  let s = 0;
  while (i < n) { s += i; i++; }
  return s;
}
console.log(plain(5));

const nested = () => {
  let total = 0;
  for (let a = 0; a < 3; a++) {
    for (let b = 0; b < 3; b++) total += a * b;
  }
  return total;
};
console.log(nested());

// The same missing merge without a loop: `propagate_frame` keeps the first frame to reach a
// join, so a local assigned in one branch reads its pre-branch value after the `if`.
const pick = (c) => {
  let x = 1;
  if (c) x = 2;
  return x;
};
console.log(pick(true), pick(false));
