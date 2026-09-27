// Tier 3 (issue 08): enrich 1M `{ price, qty, rating }` records with a computed member --
// `records.map(r => ({ ...r, total: r.price * r.qty }))`. The callback reads properties (tier 2)
// and returns a new object, which kiln's workers build in their arenas.
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, floats, qtys, ratings } from "./common.mjs";

const price = floats(N);
const qty = qtys(N);
const rating = ratings(N);
const records = [];
for (let i = 0; i < N; i += 1) records.push({ price: price[i], qty: qty[i], rating: rating[i] });
const work = MODE === "par"
  ? () => records.parallelMap((r) => ({ ...r, total: r.price * r.qty }))
  : () => records.map((r) => ({ ...r, total: r.price * r.qty }));
// `length:sum of total:sum of rating`, the last read through each new object's own keys.
function checksum(out) {
  let total = 0;
  let rated = 0;
  for (let i = 0; i < out.length; i += 1) {
    total += out[i].total;
    rated += out[i].rating;
  }
  return out.length + ":" + total + ":" + rated;
}
bench("enrich-records", work, { budgetMs: BUDGET_MS, checksum });
