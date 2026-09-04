import { bench } from "../harness.mjs";

const payload = {
  id: 1042,
  name: "kiln-benchmark",
  active: true,
  scores: [98.5, 87.2, 94.0, 100.0, 76.8],
  metadata: {
    region: "us-east-1",
    replicas: 4,
    tags: ["prod", "edge", "wasm"]
  }
};

bench("json", () => {
  let totalLen = 0;
  for (let i = 0; i < 50000; i++) {
    const str = JSON.stringify(payload);
    totalLen += str.length;
    const parsed = JSON.parse(str);
    totalLen += parsed.id;
  }
  return totalLen;
});
