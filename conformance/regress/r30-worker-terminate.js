// `worker.terminate()` (realms issue 06) stops a worker wherever it is -- in the middle of a
// loop that never yields, or idle with a listener that would keep it alive for ever -- and its
// promise settles with the exit code, 1, after the `exit` event reports the same. Terminating a
// worker that has already stopped answers `undefined`, and its `threadId` is -1 from then on.
const { Worker, isMainThread, parentPort, workerData } = require('worker_threads');

if (isMainThread) {
	(async () => {
		const busy = new Worker(__filename, { workerData: 'busy' });
		busy.on('exit', (code) => console.log('busy exit event', code));
		await new Promise((resolve) => busy.once('message', resolve));
		console.log('busy terminated', await busy.terminate());
		console.log('again', await busy.terminate(), busy.threadId);

		const idle = new Worker(__filename, { workerData: 'idle' });
		await new Promise((resolve) => idle.once('message', resolve));
		const [code] = await Promise.all([idle.terminate(), new Promise((resolve) => idle.once('exit', resolve))]);
		console.log('idle terminated', code);

		const finished = new Worker(__filename, { workerData: 'done' });
		await new Promise((resolve) => finished.once('exit', resolve));
		console.log('finished then terminated', await finished.terminate());
	})();
} else if (workerData === 'busy') {
	parentPort.postMessage('spinning');
	let n = 0;
	for (;;) n++;
} else if (workerData === 'idle') {
	parentPort.on('message', () => {});
	parentPort.postMessage('listening');
}
