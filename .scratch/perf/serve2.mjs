// The server under test. Run under node, deno, bun, or kiln; the load client is always node,
// so the runtime serving the traffic is the only variable in the comparison.
//
// The port is a constant rather than an argument: `process.argv` is not a language feature every
// backend here has, and hard-coding it keeps the program inside the surface all four share.
// Runs are sequential and the driver waits for the port to close between them.
import http from "node:http";

const body = "hello from kiln";

const server = http.createServer((req, res) => {
  res.writeHead(200, { "content-type": "text/plain" });
  res.end(body);
});

server.listen(18917, "127.0.0.1", () => {
  console.log("listening");
});
