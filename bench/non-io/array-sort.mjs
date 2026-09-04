import { bench } from "../harness.mjs";

// The array is rebuilt inside the workload on purpose: `sort` is in place, so sorting an
// already-sorted array on the second iteration would measure a different algorithm's best case.
bench("array-sort", () => {
  const arr = [];
  for (let i = 0; i < 20000; i++) {
    arr.push((i * 1103515245 + 12345) % 65536);
  }
  arr.sort((a, b) => a - b);
  return arr[0] + arr[arr.length - 1];
});
