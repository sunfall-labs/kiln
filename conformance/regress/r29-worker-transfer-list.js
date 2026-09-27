// Transfer lists between realms (realms issue 06; ADR 0070 primitive C). A transferred
// `ArrayBuffer` is detached in the sender -- `byteLength` 0, `detached` true -- and its bytes
// arrive intact, as does a view over it; one given in `workerData` with `transferList` moves the
// same way. A `MessagePort` in the list moves to the worker, and the two ends of that channel
// then talk across realms; `receiveMessageOnPort` takes from it synchronously, whichever realm
// the other end is in. A buffer in the message but not the list is copied, and an uncloneable
// value throws before anything is detached.
const { Worker, isMainThread, parentPort, workerData, MessageChannel, receiveMessageOnPort } = require('worker_threads');

if (isMainThread) {
	const initial = new ArrayBuffer(4);
	new Uint8Array(initial).set([9, 8, 7, 6]);
	const worker = new Worker(__filename, { workerData: { initial }, transferList: [initial] });
	console.log('workerData buffer detached', initial.detached, initial.byteLength);

	const buffer = new ArrayBuffer(8);
	const view = new Uint16Array(buffer, 2, 2);
	view.set([513, 1027]);
	const copied = new Uint8Array([1, 2, 3]);
	const { port1, port2 } = new MessageChannel();
	try {
		worker.postMessage({ fn() {} , buffer }, [buffer]);
	} catch (error) {
		console.log('uncloneable', error.name, buffer.byteLength);
	}
	worker.postMessage({ view, copied, port: port2 }, [buffer, port2]);
	console.log('sender', buffer.byteLength, buffer.detached, view.length, copied.length);

	port1.on('message', (message) => {
		console.log('over the transferred port', JSON.stringify(message));
		port1.postMessage('second');
		port1.postMessage('third');
	});
	worker.on('message', (message) => console.log('worker says', message));
	worker.on('exit', (code) => {
		console.log('exit', code);
		port1.close();
	});
} else {
	console.log('initial', Array.from(new Uint8Array(workerData.initial)).join(','));
	parentPort.once('message', ({ view, copied, port }) => {
		const lines = [
			`view ${view.constructor.name} ${view.byteOffset} ${Array.from(view).join(',')} of ${view.buffer.byteLength}`,
			`copied ${Array.from(copied).join(',')}`,
		];
		port.postMessage({ from: 'worker', lines });
		port.once('message', (first) => {
			// The next one is already queued behind it, and is taken without a turn of the loop.
			const next = receiveMessageOnPort(port);
			parentPort.postMessage(`${first} then ${JSON.stringify(next)} then ${receiveMessageOnPort(port)}`);
			port.close();
		});
	});
}
