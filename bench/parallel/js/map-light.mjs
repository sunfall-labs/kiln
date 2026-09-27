// map, light body: a scale and shift over a 1M-element Float64Array. Memory-bound, so it
// measures the machinery (the pool's fork-join, the output allocation), not the cores.
import { bench } from "../../harness.mjs";
import { BUDGET_MS, MODE, N, floats, sumChecksum } from "./common.mjs";

const xs = floats(N);
const work = MODE === "par"
  ? () => xs.parallelMap((x) => x * 1.5 + 2)
  : () => xs.map((x) => x * 1.5 + 2);
bench("map-light", work, { budgetMs: BUDGET_MS, checksum: sumChecksum });
