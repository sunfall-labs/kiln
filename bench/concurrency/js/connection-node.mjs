import { Worker } from "node:worker_threads";

const count = Number(process.env.BENCH_REALMS);
const workerUrl = new URL("./connection-client.mjs", import.meta.url);
const replies = await Promise.all(Array.from({ length: count }, () => new Promise((resolve, reject) => {
  const worker = new Worker(workerUrl);
  worker.once("message", (value) => resolve(value));
  worker.once("error", reject);
})));
console.log(`${replies.filter((reply) => reply === "ok").length}/${count}`);
