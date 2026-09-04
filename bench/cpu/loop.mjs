import { bench } from "../harness.mjs";

bench("loop", () => {
  let sum = 0;
  for (let i = 0; i < 50000000; i++) sum += i % 7;
  return sum;
});
