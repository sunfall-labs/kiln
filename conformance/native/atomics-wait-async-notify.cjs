// A notify wakes waitAsync even when no one changes the cell's value. The async waiter must
// share Atomics.wait's FIFO list and keep a realm alive while its promise is outstanding.
const { Worker, isMainThread, parentPort, workerData } = require('node:worker_threads');

if (isMainThread) {
  (async () => {
    const shared = new SharedArrayBuffer(4);
    const cell = new Int32Array(shared);
    const worker = new Worker(__filename, { workerData: shared });
    const exited = new Promise((resolve) => worker.once('exit', resolve));
    let answer;
    const settled = new Promise((resolve) => {
      worker.on('message', (message) => {
        if (message === 'waiting') {
          answer = Atomics.notify(cell, 0, 1);
        } else {
          resolve(message);
        }
      });
    });
    const result = await settled;
    console.log('notified', answer, result);
    console.log('exit', await exited);
    const expired = Atomics.waitAsync(cell, 0, 0, 5);
    const keepAlive = setTimeout(() => {}, 40);
    console.log('timeout', expired.async, await expired.value);
    const storeOnly = Atomics.waitAsync(cell, 0, 0, 5);
    Atomics.store(cell, 0, 1);
    console.log('store-only', storeOnly.async, await storeOnly.value);
    clearTimeout(keepAlive);
  })();
} else {
  const cell = new Int32Array(workerData);
  const pending = Atomics.waitAsync(cell, 0, 0, 2000);
  parentPort.postMessage('waiting');
  Promise.resolve(pending.value).then((answer) => parentPort.postMessage(`${pending.async} ${answer}`));
}
