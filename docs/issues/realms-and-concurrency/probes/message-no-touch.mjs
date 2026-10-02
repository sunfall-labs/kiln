// Isolate an ArrayBuffer's cross-realm handoff from the typed-array scan in the regular
// messaging benchmark. A round trip transfers the same block in both directions.
import { Worker } from "node:worker_threads";

const worker = new Worker(new URL("./message-no-touch-worker.mjs", import.meta.url));
let pending;
worker.on("message", (message) => { pending(message); });
const sizeMb = Number(process.argv[2] || 16);
let buffer = new ArrayBuffer(sizeMb * (1 << 20));
new Uint8Array(buffer)[0] = 7;
async function roundTrip() {
  const answer = new Promise((resolve) => { pending = resolve; });
  worker.postMessage(buffer, [buffer]);
  buffer = await answer;
}
for (let i = 0; i < 20; i++) await roundTrip();
const start = performance.now();
for (let i = 0; i < 200; i++) await roundTrip();
const ms = (performance.now() - start) / 200;
console.log(JSON.stringify({ case: "transfer-no-touch", size_mb: sizeMb, ms_per_op: ms,
  checksum: buffer.byteLength + ":" + new Uint8Array(buffer)[0] }));
await worker.terminate();
