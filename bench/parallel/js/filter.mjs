// filter by a predicate over a 1M-element Float64Array; about a third of the elements are kept.
// The threshold is a captured `const` (tier 1).
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, floats, sumChecksum } from "./common.mjs";

const xs = floats(N);
const cut = 250;
const work = MODE === "par"
  ? () => xs.parallelFilter((x) => Math.sin(x) * x > cut)
  : () => xs.filter((x) => Math.sin(x) * x > cut);
bench("filter", work, { budgetMs: BUDGET_MS, checksum: sumChecksum });
