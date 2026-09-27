// bench/server/serve.mjs, answering with the worker index the supervisor gave this process, so a
// client can see which of the KILN_WORKERS realms served it. Run by supervise-e2e.sh.
import http from "node:http";

const body = `worker ${process.env.KILN_WORKER_INDEX}/${process.env.KILN_WORKER_COUNT}`;

const server = http.createServer((req, res) => {
  res.writeHead(200, { "content-type": "text/plain", connection: "close" });
  res.end(body);
});

server.listen(18088, "127.0.0.1", () => {
  console.log("listening");
});
