// Tier 4 (issue 09): parse 1M CSV lines into numbers --
// `lines.parallelMap((l) => l.trim().split(",").map(Number))`. The callback reads a string (tier 2),
// builds strings and arrays (tier 3) and calls the pure built-ins `trim`, `split` and `Number`
// (tier 4), which kiln's workers run in their arenas.
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, floats, qtys } from "./common.mjs";

const price = floats(N);
const qty = qtys(N);
const lines = [];
for (let i = 0; i < N; i += 1) lines.push(` ${i},${qty[i]},${price[i]} `);
const work = MODE === "par"
  ? () => lines.parallelMap((l) => l.trim().split(",").map(Number))
  : () => lines.map((l) => l.trim().split(",").map(Number));
// `length:sum of qty:sum of price`, read back out of each parsed row.
function checksum(rows) {
  let q = 0;
  let p = 0;
  for (let i = 0; i < rows.length; i += 1) {
    q += rows[i][1];
    p += rows[i][2];
  }
  return rows.length + ":" + q + ":" + p;
}
bench("parse-csv", work, { budgetMs: BUDGET_MS, checksum });
