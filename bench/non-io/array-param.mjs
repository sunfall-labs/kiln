import { bench } from "../harness.mjs";

// The array arrives as a parameter, the idiomatic shape for a reusable kernel; module-scope
// arrays are covered by array-functional.
const data = [];
for (let i = 0; i < 4096; i++) {
  data.push((i * 7) % 101);
}

function sumSquares(values) {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    total += values[i] * values[i];
  }
  return total;
}

function countAbove(values, limit) {
  let count = 0;
  for (let i = 0; i < values.length; i++) {
    if (values[i] > limit) count++;
  }
  return count;
}

bench("array-param", () => {
  let total = 0;
  for (let iter = 0; iter < 200; iter++) {
    total += sumSquares(data) + countAbove(data, iter % 100);
  }
  return total;
});
