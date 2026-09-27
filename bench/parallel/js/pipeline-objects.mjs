// The same pipeline over records as objects -- what most code actually holds: an array of 1M
// `{ price, qty, rating }`. The score reads properties (tier 2); the filter and reduce run over
// the scores, a number array, and qualify today.
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, floats, qtys, ratings, valueChecksum } from "./common.mjs";

const price = floats(N);
const qty = qtys(N);
const rating = ratings(N);
const records = [];
for (let i = 0; i < N; i += 1) records.push({ price: price[i], qty: qty[i], rating: rating[i] });
const cut = 20000;
const work = MODE === "par"
  ? () => records
    .parallelMap((r) => Math.floor(r.price * r.qty * (5 + r.rating)))
    .parallelFilter((s) => s > cut)
    .parallelReduce((acc, s) => acc + s, (a, b) => a + b, 0)
  : () => records
    .map((r) => Math.floor(r.price * r.qty * (5 + r.rating)))
    .filter((s) => s > cut)
    .reduce((acc, s) => acc + s, 0);
bench("pipeline-objects", work, { budgetMs: BUDGET_MS, checksum: valueChecksum });
