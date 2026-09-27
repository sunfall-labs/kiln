// A worker is a realm of its own on a thread of its own (realms issue 06, ADR 0070): `new
// Worker(__filename)` runs this same file there with `isMainThread` false, and the two talk
// through `parentPort` and the `Worker` object. Messages are copies made by the structured clone,
// they arrive in the order they were sent, `online` comes before the worker's first message, and
// `exit` after its last. Until 2026-09-27 `new Worker` threw in both engines.
const { Worker, isMainThread, parentPort, threadId } = require('worker_threads');

if (isMainThread) {
	const worker = new Worker(__filename);
	console.log('main thread', threadId, 'worker thread', worker.threadId);
	const events = [];
	worker.on('online', () => events.push('online'));
	worker.on('message', (message) => {
		events.push(`message ${JSON.stringify(message)}`);
		if (message.n < 3) worker.postMessage({ n: message.n + 1, list: [message.n] });
		else worker.postMessage('bye');
	});
	worker.on('exit', (code) => {
		events.push(`exit ${code}`);
		console.log(events.join('\n'));
		console.log('threadId after exit', worker.threadId);
	});
} else {
	parentPort.on('message', (message) => {
		if (message === 'bye') {
			parentPort.close();
			return;
		}
		parentPort.postMessage({ n: message.n, echoed: message.list, from: threadId });
	});
	parentPort.postMessage({ n: 1, from: threadId });
}
