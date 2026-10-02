import { parentPort } from "node:worker_threads";

parentPort.on("message", (buffer) => parentPort.postMessage(buffer, [buffer]));
