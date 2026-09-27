// The realistic pipeline over records stored as parallel typed arrays (price, qty, rating):
// score each of 1M records (map), keep the high scores (filter), total them (reduce). The score
// reads three columns, so its callback reads the heap (`qty[i]`, `rating[i]`) -- tier 2. Today
// that stage runs sequentially and the filter and reduce run in parallel.
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, floats, qtys, ratings, valueChecksum } from "./common.mjs";

const price = floats(N);
const qty = qtys(N);
const rating = ratings(N);
const cut = 20000;
const work = MODE === "par"
  ? () => price
    .parallelMap((p, i) => Math.floor(p * qty[i] * (5 + rating[i])))
    .parallelFilter((s) => s > cut)
    .parallelReduce((acc, s) => acc + s, (a, b) => a + b, 0)
  : () => price
    .map((p, i) => Math.floor(p * qty[i] * (5 + rating[i])))
    .filter((s) => s > cut)
    .reduce((acc, s) => acc + s, 0);
bench("pipeline-records", work, { budgetMs: BUDGET_MS, checksum: valueChecksum });
