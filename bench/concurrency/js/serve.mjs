// 6. The server under test for kiln (KILN_WORKERS process mode) and single-process node: the
// same hello-world as bench/server/serve.mjs, on the port bench/concurrency owns so it does not
// collide with a bench/server run on the same machine. The port is a constant for the same
// reason serve.mjs gives: it keeps the program inside the surface every runtime shares.
import http from "node:http";

const body = "hello from kiln";

const server = http.createServer((req, res) => {
  res.writeHead(200, { "content-type": "text/plain" });
  res.end(body);
});

server.listen(18190, "127.0.0.1", () => {
  console.log("listening");
});
