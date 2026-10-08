import { bench } from "../harness.mjs";

// The array arrives as a parameter, the idiomatic shape for a reusable kernel; module-scope
// arrays are covered by array-functional.
const data: number[] = [];
for (let i: number = 0; i < 4096; i++) {
  data.push((i * 7) % 101);
}

function sumSquares(values: number[]): number {
  let total: number = 0;
  for (let i: number = 0; i < values.length; i++) {
    total += values[i] * values[i];
  }
  return total;
}

function countAbove(values: number[], limit: number): number {
  let count: number = 0;
  for (let i: number = 0; i < values.length; i++) {
    if (values[i] > limit) count++;
  }
  return count;
}

bench("array-param", (): number => {
  let total: number = 0;
  for (let iter: number = 0; iter < 200; iter++) {
    total += sumSquares(data) + countAbove(data, iter % 100);
  }
  return total;
});
