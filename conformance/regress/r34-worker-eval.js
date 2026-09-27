// `new Worker(code, { eval: true })` runs source text as the worker's program (realms issue
// 06): CommonJS, with `require` and `worker_threads` available, `workerData` and messages as
// for a file worker, and an error in the text reported as the worker's `error` event.
const { Worker } = require('worker_threads');

const source = `
const { parentPort, workerData, isMainThread } = require('worker_threads');
parentPort.on('message', (n) => {
  parentPort.postMessage({ square: n * n, base: workerData.base, isMainThread });
  parentPort.close();
});
`;

const worker = new Worker(source, { eval: true, workerData: { base: 10 } });
worker.on('message', (message) => console.log('eval worker says', JSON.stringify(message)));
worker.on('exit', (code) => {
	console.log('eval worker exit', code);
	const failing = new Worker('undefinedFunction()', { eval: true });
	failing.on('error', (error) => console.log('error', error.name, error.message));
	failing.on('exit', (failed) => console.log('failing exit', failed));
});
worker.postMessage(7);
