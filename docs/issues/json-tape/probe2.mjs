const payload = { id: 1042, name: "kiln-benchmark", active: true, scores: [98.5, 87.2, 94.0, 100.0, 76.8], metadata: { region: "us-east-1", replicas: 4, tags: ["prod", "edge", "wasm"] } };
const text = JSON.stringify(payload);
console.log(text);
for (let i = 0; i < 3; i++) {
  const p = JSON.parse(text);
  console.log(i, typeof p, p && Object.keys(p).join(","), p && p.id, p && p.scores && p.scores.length, JSON.stringify(p));
}
console.log(JSON.stringify(JSON.parse("[98.5, 87.2, 94, 1e3, -0.5]")));
let total = 0;
for (let i = 0; i < 1000; i++) { const parsed = JSON.parse(text); total += parsed.id + parsed.scores.length + parsed.metadata.replicas; }
console.log("total", total);
