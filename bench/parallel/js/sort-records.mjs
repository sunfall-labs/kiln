// sort 1M records by a key, `(a, b) => a.price - b.price`: the comparator reads a property of
// each element (tier 2). As in sort-array, every call sorts a fresh copy, and the copy is timed.
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, floats, recordsChecksum } from "./common.mjs";

const price = floats(N);
const records = [];
for (let i = 0; i < N; i += 1) records.push({ id: i, price: price[i] });
const work = MODE === "par"
  ? () => records.slice().parallelSort((a, b) => a.price - b.price)
  : () => records.slice().sort((a, b) => a.price - b.price);
bench("sort-records", work, { budgetMs: BUDGET_MS, checksum: recordsChecksum });
