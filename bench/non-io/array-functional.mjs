import { bench } from "../harness.mjs";

// Built once, outside the timed region: allocating it per iteration would measure the
// allocation rather than map/filter/reduce, which is what this is for.
const numbers = [];
for (let i = 0; i < 5000; i++) {
  numbers.push(i);
}

bench("array-functional", () => {
  let total = 0;
  for (let iter = 0; iter < 100; iter++) {
    const result = numbers
      .map(x => x * 2 + 1)
      .filter(x => x % 3 === 0)
      .reduce((acc, x) => acc + x, 0);
    total += result;
  }
  return total;
});
