// An exception nothing in a worker catches ends the worker (realms issue 06): the parent's
// `error` event gets a copy of it -- the same kind of error, its message -- and then `exit`
// reports 1. The parent itself carries on. That holds for a throw from the module body, from a
// callback the worker's event loop runs later, and for a rejection nothing handled.
const { Worker, isMainThread, parentPort, workerData } = require('worker_threads');

if (isMainThread) {
	(async () => {
		for (const where of ['body', 'timer', 'rejection', 'value']) {
			const worker = new Worker(__filename, { workerData: where });
			worker.on('message', (message) => console.log(where, 'message', message));
			worker.on('error', (error) => {
				if (error instanceof Error) {
					console.log(where, 'error', error.constructor.name, error.name, JSON.stringify(error.message), error instanceof TypeError);
				} else {
					console.log(where, 'error value', typeof error, error);
				}
			});
			const code = await new Promise((resolve) => worker.once('exit', resolve));
			console.log(where, 'exit', code);
		}
		console.log('parent still running');
	})();
} else if (workerData === 'body') {
	parentPort.postMessage('before the throw');
	throw new TypeError('thrown from the body');
} else if (workerData === 'timer') {
	setTimeout(() => {
		throw new RangeError('thrown from a timer');
	}, 1);
} else if (workerData === 'rejection') {
	Promise.reject(new SyntaxError('rejected and never handled'));
} else if (workerData === 'value') {
	throw 42;
}
