// One realm or worker per TCP connection. The Python harness owns the loopback server.
import net from "node:net";
import { parentPort } from "node:worker_threads";

const socket = net.connect(Number(process.env.BENCH_PORT), "127.0.0.1");
socket.on("connect", () => socket.write("ping"));
socket.on("data", (chunk) => {
  const result = chunk.toString() === "pong" ? "ok" : "wrong";
  if (parentPort) parentPort.postMessage(result);
  else console.log(result);
  socket.end();
});
socket.on("error", (error) => {
  const result = `error ${error.message}`;
  if (parentPort) parentPort.postMessage(result);
  else console.log(result);
});
