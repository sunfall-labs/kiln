// reduce, a heavier fold: each element goes through eight rounds of an integer mixing step
// before it is added. Integer-valued throughout, so exact in any order.
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, ints, valueChecksum } from "./common.mjs";

const xs = ints(N);
const work = MODE === "par"
  ? () => xs.parallelReduce((acc, x) => {
    let y = x;
    for (let k = 0; k < 8; k += 1) y = (y * 31 + k) % 1000003;
    return acc + y;
  }, (a, b) => a + b, 0)
  : () => xs.reduce((acc, x) => {
    let y = x;
    for (let k = 0; k < 8; k += 1) y = (y * 31 + k) % 1000003;
    return acc + y;
  }, 0);
bench("reduce-fold", work, { budgetMs: BUDGET_MS, checksum: valueChecksum });
