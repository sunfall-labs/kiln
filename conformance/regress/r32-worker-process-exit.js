// `process.exit()` in a worker ends that worker, not the process (realms issue 06): the parent's
// `exit` event reports the code, and the parent goes on running. Nothing after the call runs in
// the worker -- not the rest of the handler, not a pending timer -- while what it posted before
// the call is still delivered, ahead of the exit.
const { Worker, isMainThread, parentPort, workerData } = require('worker_threads');

if (isMainThread) {
	(async () => {
		for (const where of ['body', 'handler']) {
			const worker = new Worker(__filename, { workerData: where });
			worker.on('message', (message) => console.log(where, 'message', message));
			if (where === 'handler') worker.postMessage('please exit');
			const code = await new Promise((resolve) => worker.once('exit', resolve));
			console.log(where, 'exit', code);
		}
		setTimeout(() => console.log('parent still running'), 10);
	})();
} else if (workerData === 'body') {
	setTimeout(() => parentPort.postMessage('timer ran'), 50);
	parentPort.postMessage('posted before exit');
	process.exit(3);
	parentPort.postMessage('after exit');
} else {
	parentPort.on('message', (message) => {
		parentPort.postMessage(`got ${message}`);
		process.exit(7);
		parentPort.postMessage('after exit');
	});
}
