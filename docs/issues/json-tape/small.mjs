// JSON.parse on the bench/non-io/json.mjs payload (~200 bytes), parse only.
import { bench } from "../../bench/harness.mjs";

const payload = {
  id: 1042,
  name: "kiln-benchmark",
  active: true,
  scores: [98.5, 87.2, 94.0, 100.0, 76.8],
  metadata: { region: "us-east-1", replicas: 4, tags: ["prod", "edge", "wasm"] }
};
const text = JSON.stringify(payload);

bench("json-parse-small", () => {
  let total = 0;
  for (let i = 0; i < 1000; i++) {
    const parsed = JSON.parse(text);
    total += parsed.id + parsed.scores.length + parsed.metadata.replicas;
  }
  return total;
}, { externalIterations: 200 });
