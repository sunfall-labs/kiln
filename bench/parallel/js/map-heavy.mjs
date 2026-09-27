// map, vector maths: a few transcendental calls per element over a 1M-element Float64Array.
// Compute-bound; the case parallelism is for.
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, floats, sumChecksum } from "./common.mjs";

const xs = floats(N);
const work = MODE === "par"
  ? () => xs.parallelMap((x) => Math.sqrt(x * x + 1) * Math.sin(x) + Math.cos(x * 0.5))
  : () => xs.map((x) => Math.sqrt(x * x + 1) * Math.sin(x) + Math.cos(x * 0.5));
bench("map-heavy", work, { budgetMs: BUDGET_MS, checksum: sumChecksum });
