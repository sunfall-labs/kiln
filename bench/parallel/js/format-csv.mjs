// Tier 3 (issue 08): format 1M records as CSV lines --
// `records.map(r => `${r.id},${r.qty},${r.price}`)`. Number formatting and concatenation
// allocate strings, which kiln's workers build in their arenas. (Parsing CSV needs `split` and
// `Number`, tier 4, and waits below.)
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, floats, qtys } from "./common.mjs";

const price = floats(N);
const qty = qtys(N);
const records = [];
for (let i = 0; i < N; i += 1) records.push({ id: i, qty: qty[i], price: price[i] });
const work = MODE === "par"
  ? () => records.parallelMap((r) => `${r.id},${r.qty},${r.price}`)
  : () => records.map((r) => `${r.id},${r.qty},${r.price}`);
// `length:total characters:sum of the character codes at a sample of positions`.
function checksum(lines) {
  let chars = 0;
  let codes = 0;
  for (let i = 0; i < lines.length; i += 1) {
    chars += lines[i].length;
    if (i % 997 === 0) codes += lines[i].charCodeAt(lines[i].length - 1);
  }
  return lines.length + ":" + chars + ":" + codes;
}
bench("format-csv", work, { budgetMs: BUDGET_MS, checksum });
