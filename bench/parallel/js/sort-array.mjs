// sort a 1M-element array of numbers with `(a, b) => a - b`. As in sort-typed, every call sorts
// a fresh copy, and the copy is timed.
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, floats, sortChecksum } from "./common.mjs";

const xs = Array.from(floats(N));
const work = MODE === "par"
  ? () => xs.slice().parallelSort((a, b) => a - b)
  : () => xs.slice().sort((a, b) => a - b);
bench("sort-array", work, { budgetMs: BUDGET_MS, checksum: sortChecksum });
