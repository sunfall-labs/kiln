// A worker runs the same CommonJS entry in its own realm. The parent observes online,
// messages, and exit in order, and the worker sees its cloned workerData.
const { Worker, isMainThread, parentPort, workerData, threadId } = require('node:worker_threads');

if (isMainThread) {
  const worker = new Worker(__filename, { workerData: { value: 41 } });
  const events = [];
  worker.on('online', () => events.push('online'));
  worker.on('message', (value) => {
    events.push(`message:${value}`);
    worker.postMessage('close');
  });
  worker.on('error', (error) => { throw error; });
  worker.on('exit', (code) => {
    events.push(`exit:${code}`);
    if (events.join(',') !== 'online,message:42,exit:0' || worker.threadId !== -1) {
      throw new Error(`worker lifecycle: ${events.join(',')} threadId=${worker.threadId}`);
    }
    console.log('pass');
  });
} else {
  if (threadId <= 0 || workerData.value !== 41) throw new Error('worker realm/data');
  parentPort.on('message', (value) => {
    if (value !== 'close') throw new Error('worker message');
    parentPort.close();
  });
  parentPort.postMessage(workerData.value + 1);
}
