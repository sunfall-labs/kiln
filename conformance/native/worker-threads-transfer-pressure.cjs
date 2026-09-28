// A backing store can cross the same two realms repeatedly, while separate stores sent later
// become unreachable at the receiver. Both cases exercise ownership and collection accounting.
const { Worker, isMainThread, parentPort } = require("node:worker_threads");

if (isMainThread) {
  const worker = new Worker(__filename);
  let buffer = new ArrayBuffer(1 << 20);
  new Uint8Array(buffer)[0] = 7;
  let bounced = 0;
  let distinct = 0;
  let sum = 0;
  worker.on("message", (message) => {
    if (message.buffer) {
      buffer = message.buffer;
      sum += message.byte;
      bounced += 1;
      if (bounced < 12) {
        worker.postMessage({ kind: "bounce", step: bounced, buffer }, [buffer]);
      } else {
        for (let i = 0; i < 12; i += 1) {
          const fresh = new ArrayBuffer(1 << 20);
          new Uint8Array(fresh)[0] = i;
          worker.postMessage({ kind: "drop", buffer: fresh }, [fresh]);
          if (fresh.byteLength !== 0) throw new Error("transfer did not detach");
        }
      }
    } else {
      sum += message.byte;
      distinct += 1;
      if (distinct === 12) {
        worker.terminate().then(() => console.log(bounced, distinct, sum, buffer.byteLength));
      }
    }
  });
  worker.postMessage({ kind: "bounce", step: 0, buffer }, [buffer]);
} else {
  parentPort.on("message", (message) => {
    const byte = new Uint8Array(message.buffer)[0];
    if (message.kind === "bounce") {
      let buffer = message.buffer;
      if (message.step === 3) buffer = buffer.transfer(2 << 20);
      if (message.step === 7) buffer = buffer.transfer(1 << 20);
      parentPort.postMessage({ buffer, byte }, [buffer]);
    } else {
      parentPort.postMessage({ byte });
    }
  });
}
