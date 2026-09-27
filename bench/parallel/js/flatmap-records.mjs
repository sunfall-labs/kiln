// Tier 3 (issue 08): flatten 1M records into a number array, two entries for the larger orders
// and none for the rest -- `records.flatMap(r => r.qty > 4 ? [r.price, r.qty] : [])`. Each
// callback answer is a new array, which kiln's workers build in their arenas; the realm thread
// flattens them.
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, floats, qtys, sumChecksum } from "./common.mjs";

const price = floats(N);
const qty = qtys(N);
const records = [];
for (let i = 0; i < N; i += 1) records.push({ price: price[i], qty: qty[i] });
const work = MODE === "par"
  ? () => records.parallelFlatMap((r) => (r.qty > 4 ? [r.price, r.qty] : []))
  : () => records.flatMap((r) => (r.qty > 4 ? [r.price, r.qty] : []));
bench("flatmap-records", work, { budgetMs: BUDGET_MS, checksum: sumChecksum });
