// Plain cross-realm graph decoding keeps cycles, holes, key identity and the own __proto__
// data property. Accessors and transferred buffers use the shared TypeScript fallback.
const { Worker, isMainThread, parentPort } = require("node:worker_threads");

if (isMainThread) {
  const worker = new Worker(__filename);
  const answers = [];
  worker.on("message", (answer) => answers.push(answer));
  const graph = JSON.parse('{"__proto__":7,"name":"𝄞"}');
  graph.self = graph;
  graph.items = [, graph];
  graph.rows = [];
  for (let i = 0; i < 64; i++) graph.rows.push({ i, text: String(i) });
  worker.postMessage({ kind: "graph", graph });

  let reads = 0;
  const accessor = { get answer() { reads += 1; return 42; } };
  worker.postMessage({ kind: "getter", accessor });

  const buffer = new ArrayBuffer(16);
  new Uint8Array(buffer)[15] = 9;
  worker.postMessage({ kind: "transfer", buffer }, [buffer]);
  const detached = buffer.byteLength;

  worker.postMessage({ kind: "finish" });

  worker.on("exit", () => {
    console.log(answers.join("\n"));
    console.log("sender", reads, detached);
  });
} else {
  parentPort.on("message", (message) => {
    if (message.kind === "finish") {
      parentPort.close();
      return;
    }
    if (message.kind === "graph") {
      const value = message.graph;
      parentPort.postMessage([value.self === value, value.items[1] === value,
        0 in value.items, value.name, Object.hasOwn(value, "__proto__"), value.__proto__,
        value.rows.length, value.rows[0].i, value.rows[63].text].join(" "));
    } else if (message.kind === "getter") {
      parentPort.postMessage("getter " + message.accessor.answer);
    } else if (message.kind === "transfer") {
      parentPort.postMessage("transfer " + message.buffer.byteLength + " " + new Uint8Array(message.buffer)[15]);
    }
  });
}
