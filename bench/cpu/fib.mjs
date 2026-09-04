import { bench } from "../harness.mjs";

function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }

bench("fib", () => {
  let total = 0;
  for (let i = 0; i < 5; i++) total += fib(30);
  return total;
});
