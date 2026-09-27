// What a worker starts from (realms issue 06): `workerData` is a structured clone of the value
// given to the constructor -- a Map, a Date, a cycle and a shared reference arrive as such --
// taken when the worker is made, so a later change in the parent is not seen. `argv` is
// appended to the worker's `process.argv`, `env` replaces its `process.env`, `name` is its
// `threadName`, `resourceLimits` are reported on both sides with node's defaults for the ones not
// given, and data set with `setEnvironmentData` before the worker starts is a copy it can read.
const {
	Worker, isMainThread, parentPort, workerData, threadId, threadName, resourceLimits,
	getEnvironmentData, setEnvironmentData, SHARE_ENV,
} = require('worker_threads');

if (isMainThread) {
	const shared = { label: 'shared' };
	const data = { map: new Map([['k', 1]]), when: new Date(0), list: [1, 'two', null], a: shared, b: shared };
	data.self = data;
	setEnvironmentData('config', { mode: 'test' });
	const worker = new Worker(__filename, {
		workerData: data,
		argv: ['first', 2],
		env: { ONLY: 'this' },
		name: 'reporter',
		resourceLimits: { maxOldGenerationSizeMb: 64 },
	});
	data.list.push('added after');
	setEnvironmentData('config', 'changed after');
	console.log('parent limits', JSON.stringify(worker.resourceLimits));
	console.log('main', isMainThread, threadId, JSON.stringify(threadName), JSON.stringify(resourceLimits), typeof SHARE_ENV);
	worker.on('message', (report) => console.log(report.join('\n')));
	worker.on('exit', (code) => console.log('exit', code));
} else {
	parentPort.postMessage([
		`isMainThread ${isMainThread}`,
		`threadId ${threadId}`,
		`threadName ${threadName}`,
		`map ${workerData.map instanceof Map} ${workerData.map.get('k')}`,
		`date ${workerData.when instanceof Date} ${workerData.when.getTime()}`,
		`list ${JSON.stringify(workerData.list)}`,
		`shared ${workerData.a === workerData.b} ${workerData.a.label}`,
		`cycle ${workerData.self === workerData}`,
		`argv ${JSON.stringify(process.argv.slice(2))}`,
		`env ${JSON.stringify(process.env)}`,
		`limits ${JSON.stringify(resourceLimits)}`,
		`environmentData ${JSON.stringify(getEnvironmentData('config'))}`,
	]);
}
