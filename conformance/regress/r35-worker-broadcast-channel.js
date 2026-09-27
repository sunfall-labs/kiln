// A `BroadcastChannel` reaches every other channel of the same name in the process, in this
// realm and in every worker's (realms issue 06): each receiver gets its own copy, the sender
// none, and a channel that is closed hears nothing more. Until 2026-09-27 a channel reached
// only channels in its own realm.
const { Worker, isMainThread, parentPort, workerData, BroadcastChannel } = require('worker_threads');

if (isMainThread) {
	const channel = new BroadcastChannel('news');
	const local = new BroadcastChannel('news');
	const heard = [];
	local.onmessage = (event) => heard.push(`local heard ${JSON.stringify(event.data)}`);
	const workers = [1, 2].map((n) => new Worker(__filename, { workerData: n }));
	let ready = 0;
	let reports = 0;
	for (const worker of workers) {
		worker.on('message', (message) => {
			if (message === 'ready') {
				if (++ready === workers.length) channel.postMessage({ headline: 'hello', items: [1, 2] });
				return;
			}
			heard.push(message);
			if (++reports === workers.length) {
				heard.sort();
				console.log(heard.join('\n'));
				channel.close();
				local.close();
			}
		});
	}
} else {
	const channel = new BroadcastChannel('news');
	channel.onmessage = (event) => {
		parentPort.postMessage(`worker ${workerData} heard ${JSON.stringify(event.data)}`);
		channel.close();
	};
	parentPort.postMessage('ready');
}
