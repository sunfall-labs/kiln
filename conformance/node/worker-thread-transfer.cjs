// Node's transfer list detaches the sender's ArrayBuffer and delivers its bytes.
const { Worker, isMainThread, parentPort, workerData } = require('node:worker_threads');

if (isMainThread) {
  const bytes = new Uint8Array([3, 5, 8]);
  const worker = new Worker(__filename, {
    workerData: bytes.buffer,
    transferList: [bytes.buffer],
  });
  if (bytes.buffer.byteLength !== 0) throw new Error('sender buffer was not detached');
  let received = false;
  worker.on('message', (sum) => {
    if (sum !== 16) throw new Error(`worker sum: ${sum}`);
    received = true;
  });
  worker.on('error', (error) => { throw error; });
  worker.on('exit', (code) => {
    if (code !== 0 || !received) throw new Error(`worker exit: ${code}, received=${received}`);
    console.log('pass');
  });
} else {
  const bytes = new Uint8Array(workerData);
  if (bytes.length !== 3) throw new Error('worker buffer length');
  parentPort.postMessage(bytes[0] + bytes[1] + bytes[2]);
}
