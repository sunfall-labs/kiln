// reduce, a plain sum over 1M integer-valued doubles. Exact in any order, so the parallel tree
// and the sequential fold must agree to the last digit.
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, ints, valueChecksum } from "./common.mjs";

const xs = ints(N);
const work = MODE === "par"
  ? () => xs.parallelReduce((acc, x) => acc + x, (a, b) => a + b, 0)
  : () => xs.reduce((acc, x) => acc + x, 0);
bench("reduce-sum", work, { budgetMs: BUDGET_MS, checksum: valueChecksum });
