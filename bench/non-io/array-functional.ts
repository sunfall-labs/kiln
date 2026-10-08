import { bench } from "../harness.mjs";

// Built once, outside the timed region: allocating it per iteration would measure the
// allocation rather than map/filter/reduce, which is what this is for.
const numbers: number[] = [];
for (let i: number = 0; i < 5000; i++) {
  numbers.push(i);
}

bench("array-functional", (): number => {
  let total: number = 0;
  for (let iter: number = 0; iter < 100; iter++) {
    const result: number = numbers
      .map((x: number): number => x * 2 + 1)
      .filter((x: number): boolean => x % 3 === 0)
      .reduce((acc: number, x: number): number => acc + x, 0);
    total += result;
  }
  return total;
});
