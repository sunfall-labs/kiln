import { parentPort } from "node:worker_threads";
import { touchBuffer, walkGraph } from "./common.mjs";

let counting = 0;
let counted = 0;
let total = 0;
parentPort.on("message", (message) => {
  if (counting > 0) {
    total += message.qty;
    counted += 1;
    if (counted === counting) {
      counting = 0;
      parentPort.postMessage(total);
    }
    return;
  }
  switch (message.kind) {
    case "echo":
      parentPort.postMessage(message);
      break;
    case "count-start":
      counting = message.count;
      counted = 0;
      total = 0;
      break;
    case "walk":
      parentPort.postMessage(walkGraph(message.graph));
      break;
    case "buffer": {
      const check = touchBuffer(new Uint8Array(message.buffer));
      parentPort.postMessage({ buffer: message.buffer, check }, message.transfer ? [message.buffer] : undefined);
      break;
    }
  }
});
