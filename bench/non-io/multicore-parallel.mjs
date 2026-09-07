import { bench } from "../harness.mjs";
// node and bun have no `parallelMap`; kiln describes it as an array member on both engines
// (ADR 0054), so a feature test is the honest guard and the assignment never runs there.
if (typeof Array.prototype.parallelMap !== "function") {
  Array.prototype.parallelMap = function (fn) {
    return this.map(fn);
  };
}


const numbers = [];
for (let i = 0; i < 5000; i++) {
  numbers.push(i);
}

bench("multicore-parallel", () => {
  let total = 0;
  for (let iter = 0; iter < 10; iter++) {
    const mapped = numbers.parallelMap(x => x * 2 + 1);
    const result = mapped
      .filter(x => x % 3 === 0)
      .reduce((acc, x) => acc + x, 0);
    total += result;
  }
  return total;
});
