// A worker whose script is a separate module of the program, named through the
// `node:worker_threads` binding under another name. Every other worker probe spawns itself
// (`new URL(import.meta.url)` or `__filename`), which a native build always links; this one
// asks that the compiler link the named target, and that both engines run it.
import { Worker as Thread } from 'node:worker_threads';
import * as threads from 'node:worker_threads';

function run(worker) {
  return new Promise((resolve) => {
    const seen = [];
    worker.on('message', (message) => seen.push(message));
    worker.on('error', (error) => seen.push(`error ${error.message}`));
    worker.on('exit', (code) => { seen.push(`exit ${code}`); resolve(seen); });
  });
}

console.log(await run(new Thread(new URL('./support/worker-target-renamed.mjs', import.meta.url), {
  workerData: { label: 'renamed', value: 21 },
})));
console.log(await run(new threads.Worker(new URL('./support/worker-target-namespace.mjs', import.meta.url), {
  workerData: { label: 'namespace', value: 4 },
})));
