// bench/server/serve.mjs, answering with the worker index the supervisor gave this process, so a
// client can see which of the KILN_WORKERS realms served it. Run by supervise-e2e.sh.
// PROBE_EXIT_WORKER=<i> makes worker i exit with status 1 shortly after it starts listening.
import http from "node:http";

const index = process.env.KILN_WORKER_INDEX;
const body = `worker ${index}/${process.env.KILN_WORKER_COUNT}`;

const server = http.createServer((req, res) => {
  res.writeHead(200, { "content-type": "text/plain", connection: "close" });
  res.end(body);
});

server.listen(18088, "127.0.0.1", () => {
  console.log("listening");
  if (index !== undefined && process.env.PROBE_EXIT_WORKER === index) {
    setTimeout(() => process.exit(1), 500);
  }
});
