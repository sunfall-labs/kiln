// Target for workers-module-target.mjs (namespace import): a worker module other than the probe itself.
import { parentPort, workerData } from 'node:worker_threads';
parentPort.postMessage(`${workerData.label} ran with ${workerData.value * 2}`);
