// JSON.parse on a ~1 MB document: 10k same-shaped objects with strings, floats, nested arrays.
import { bench } from "../../bench/harness.mjs";

const rows = [];
for (let i = 0; i < 10000; i++) {
  rows.push({
    id: i,
    name: "user-" + i + "-élève",
    email: "user" + i + "@example.com",
    score: i * 1.5 + 0.25,
    active: (i & 1) === 0,
    tags: ["alpha", "beta", "gamma-" + (i % 7)],
    address: { city: "Springfield", zip: 10000 + (i % 900), geo: [40.7 + i / 1e6, -74.0 - i / 1e6] },
    note: null
  });
}
const text = JSON.stringify(rows);

bench("json-parse-large", () => {
  const parsed = JSON.parse(text);
  return parsed.length + parsed[9999].address.zip + text.length;
}, { externalIterations: 20 });
