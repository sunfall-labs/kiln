// A realistic pipeline over one column: convert 1M order amounts to whole cents in another
// currency with tax (map), keep the orders over 500.00 (filter), total them (reduce). Every
// callback is tier 1: numbers in, captured constants, `Math` only.
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, floats, valueChecksum } from "./common.mjs";

const amounts = floats(N);
const fx = 1.0873;
const taxed = 1.2;
const cut = 50000;
const work = MODE === "par"
  ? () => amounts
    .parallelMap((x) => Math.floor(x * fx * taxed * 100))
    .parallelFilter((c) => c > cut)
    .parallelReduce((acc, c) => acc + c, (a, b) => a + b, 0)
  : () => amounts
    .map((x) => Math.floor(x * fx * taxed * 100))
    .filter((c) => c > cut)
    .reduce((acc, c) => acc + c, 0);
bench("pipeline-column", work, { budgetMs: BUDGET_MS, checksum: valueChecksum });
