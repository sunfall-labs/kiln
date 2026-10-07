import { bench } from "../harness.mjs";

const historicalEntries = 50000;
const map = new Map();
const set = new Set();
for (let i = 0; i < historicalEntries; i += 1) {
  map.set(i, i);
  set.add(i);
}
for (let i = 0; i < historicalEntries; i += 1) {
  map.delete(i);
  set.delete(i);
}

let callbacks = 0;
const noop = () => { callbacks += 1; };

bench("map-forEach-deleted-history", () => {
  map.forEach(noop);
  return callbacks;
}, { checksum: () => callbacks });

bench("set-forEach-deleted-history", () => {
  set.forEach(noop);
  return callbacks;
}, { checksum: () => callbacks });
