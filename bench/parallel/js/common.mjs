// Data and checksums every program in bench/parallel shares -- node, bun, the worker_threads
// pool, and the kiln native binaries -- and which go/main.go reproduces bit for bit.
//
// Every value comes from one integer hash, so the four runtimes build identical inputs without
// shipping a data file: h(i) = (i + 1) * 2654435761 mod 2^32 (Knuth's multiplicative hash).
// `Math.imul` does the wrap-around multiply in JavaScript; Go's uint32 arithmetic does it there.
//
// Checksums are computed once, after the clock has stopped (`bench`'s `checksum` option), in
// index order, and printed as `length:sum`. run.py compares them at twelve significant digits:
// enough to catch any element that is wrong, loose enough to forgive the last-ulp differences
// between libm's and Go's `sin`. The folds and pipelines are integer-valued, so their sums are
// exact in every runtime whatever order a parallel reduction adds them in.

export const N = Number(process.argv[3] ?? 1000000);
export const BUDGET_MS = Number(process.argv[4] ?? 1500);
/// `seq` (the ordinary method) or `par` (kiln's parallel method). node and bun only run `seq`.
export const MODE = process.argv[2] ?? "seq";

export function hash(i) {
  return Math.imul(i + 1, 2654435761) >>> 0;
}

/// n uniformly spread doubles in [0, 1000).
export function floats(n) {
  const out = new Float64Array(n);
  for (let i = 0; i < n; i += 1) out[i] = (hash(i) / 4294967296) * 1000;
  return out;
}

/// n integers in [0, 100003), stored as doubles.
export function ints(n) {
  const out = new Float64Array(n);
  for (let i = 0; i < n; i += 1) out[i] = hash(i) % 100003;
  return out;
}

/// The record columns: price as floats(n), qty in 1..9, rating in 1..5.
export function qtys(n) {
  const out = new Float64Array(n);
  for (let i = 0; i < n; i += 1) out[i] = 1 + ((hash(i) >>> 7) % 9);
  return out;
}

export function ratings(n) {
  const out = new Float64Array(n);
  for (let i = 0; i < n; i += 1) out[i] = 1 + ((hash(i) >>> 13) % 5);
  return out;
}

/// `length:sum`, the sum in index order.
export function sumChecksum(values) {
  let sum = 0;
  for (let i = 0; i < values.length; i += 1) sum += values[i];
  return values.length + ":" + sum;
}

/// A sorted result: `length:first:last:weighted sample`. The sample weights every 997th element
/// by its position class, so a result that is sorted differently cannot keep the same sum.
export function sortChecksum(values) {
  let sum = 0;
  for (let i = 0; i < values.length; i += 997) sum += values[i] * ((i % 13) + 1);
  return values.length + ":" + values[0] + ":" + values[values.length - 1] + ":" + sum;
}

/// Records sorted by `price`: `sortChecksum` of their prices, in the result's order.
export function recordsChecksum(records) {
  const prices = new Float64Array(records.length);
  for (let i = 0; i < records.length; i += 1) prices[i] = records[i].price;
  return sortChecksum(prices);
}

/// A number that is already the answer (a fold, a pipeline's total).
export function valueChecksum(value) {
  return "1:" + value;
}
