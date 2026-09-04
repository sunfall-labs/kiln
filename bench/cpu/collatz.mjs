import { bench } from "../harness.mjs";

bench("collatz", () => {
  let steps = 0;
  for (let n = 1; n < 300000; n++) {
    let m = n;
    while (m !== 1) {
      m = m % 2 === 0 ? m / 2 : 3 * m + 1;
      steps += 1;
    }
  }
  return steps;
});
