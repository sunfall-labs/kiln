// sort a 1M-element Float64Array in its default (numeric) order. Sorting destroys its input, so
// every call sorts a fresh copy; the copy is inside the timing for every runtime alike.
//
// The copy is `new Float64Array(xs)`, not `xs.slice()`: at 4819906f kiln's typed-array `slice`
// is a sequential TypeScript built-in costing ~200 ms per million elements -- ten times the
// parallel sort it would precede -- so a `slice` here would measure `slice`.
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, floats, sortChecksum } from "./common.mjs";

const xs = floats(N);
const work = MODE === "par"
  ? () => new Float64Array(xs).parallelSort()
  : () => new Float64Array(xs).sort();
bench("sort-typed", work, { budgetMs: BUDGET_MS, checksum: sortChecksum });
