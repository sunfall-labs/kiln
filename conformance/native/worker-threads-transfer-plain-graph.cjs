// Transferred backing stores keep graph identity. A getter and an untransferable buffer take
// the shared clone path before any sender-side detachment.
const { Worker, MessageChannel, isMainThread, parentPort, markAsUntransferable } = require("node:worker_threads");

if (isMainThread) {
  const worker = new Worker(__filename);
  const answers = [];
  worker.on("message", (answer) => answers.push(answer));
  const buffer = new ArrayBuffer(64);
  new Uint8Array(buffer)[63] = 9;
  const graph = { buffer, alias: buffer };
  graph.self = graph;
  worker.postMessage({ kind: "graph", graph }, [buffer]);
  const detachedGraph = buffer.byteLength;

  const fallback = new ArrayBuffer(8);
  new Uint8Array(fallback)[7] = 4;
  let reads = 0;
  const value = { get buffer() { reads++; return fallback; } };
  worker.postMessage({ kind: "getter", value }, [fallback]);
  const detachedFallback = fallback.byteLength;

  const marked = new ArrayBuffer(8);
  markAsUntransferable(marked);
  let error = "none";
  try { worker.postMessage({ buffer: marked }, [marked]); }
  catch (caught) { error = caught.name; }
  const unreachable = new ArrayBuffer(12);
  worker.postMessage({ kind: "unreachable" }, [unreachable]);
  const detachedUnreachable = unreachable.byteLength;

  const channel = new MessageChannel();
  channel.port1.close();
  const closed = new ArrayBuffer(6);
  channel.port1.postMessage({ closed }, [closed]);
  const detachedClosed = closed.byteLength;
  channel.port2.close();

  worker.postMessage({ kind: "finish" });
  worker.on("exit", () => {
    console.log(answers.join("\n"));
    console.log("sender", detachedGraph, reads, detachedFallback, error, marked.byteLength,
      detachedUnreachable, detachedClosed);
  });
} else {
  parentPort.on("message", (message) => {
    if (message.kind === "finish") {
      parentPort.close();
    } else if (message.kind === "graph") {
      const graph = message.graph;
      parentPort.postMessage([graph.self === graph, graph.buffer === graph.alias,
        graph.buffer.byteLength, new Uint8Array(graph.buffer)[63]].join(" "));
    } else if (message.kind === "getter") {
      parentPort.postMessage("getter " + message.value.buffer.byteLength + " " +
        new Uint8Array(message.value.buffer)[7]);
    } else if (message.kind === "unreachable") {
      parentPort.postMessage("unreachable");
    }
  });
}
