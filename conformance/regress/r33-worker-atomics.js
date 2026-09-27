// A `SharedArrayBuffer` is the same memory in every realm it is sent to, not a copy (realms
// issue 06): four workers adding to one counter with `Atomics.add` leave exactly their total,
// and `Atomics.wait` in a worker sleeps until the parent's `Atomics.notify` on the same cell --
// "ok", where a worker whose expected value is already stale answers "not-equal" at once.
const { Worker, isMainThread, parentPort, workerData } = require('worker_threads');

const ADDS = 2000;

if (isMainThread) {
	(async () => {
		const shared = new SharedArrayBuffer(16);
		const cells = new Int32Array(shared);
		const adders = [0, 1, 2, 3].map(() => new Worker(__filename, { workerData: { shared, role: 'add' } }));
		await Promise.all(adders.map((worker) => new Promise((resolve) => worker.once('exit', resolve))));
		console.log('counter', Atomics.load(cells, 0), 'expected', 4 * ADDS);

		const waiter = new Worker(__filename, { workerData: { shared, role: 'wait' } });
		const answers = [];
		waiter.on('message', (message) => {
			if (message === 'waiting') {
				// Spin until the worker is really asleep on the cell, then wake it.
				const poll = () => {
					if (Atomics.load(cells, 2) !== 1) return setTimeout(poll, 1);
					Atomics.store(cells, 1, 1);
					answers.push(`woken ${Atomics.notify(cells, 1, 1)}`);
				};
				poll();
			} else {
				answers.push(message);
			}
		});
		await new Promise((resolve) => waiter.once('exit', resolve));
		console.log(answers.join('\n'));
		console.log('shared bytes seen by the parent', Array.from(new Uint8Array(shared, 12, 4)).join(','));
	})();
} else {
	const cells = new Int32Array(workerData.shared);
	if (workerData.role === 'add') {
		for (let i = 0; i < ADDS; i++) Atomics.add(cells, 0, 1);
	} else {
		parentPort.postMessage('waiting');
		// Announce the wait from inside the same critical section as the value it compares.
		Atomics.store(cells, 2, 1);
		const first = Atomics.wait(cells, 1, 0);
		const second = Atomics.wait(cells, 1, 0);
		new Uint8Array(workerData.shared, 12, 4).set([1, 2, 3, 4]);
		parentPort.postMessage(`first wait ${first}, second wait ${second}`);
	}
}
