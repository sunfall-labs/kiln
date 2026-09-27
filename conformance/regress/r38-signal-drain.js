// Graceful shutdown (realms issue 23): a `SIGTERM` listener that closes the server lets the
// request in flight finish -- node's `close` callback waits for the last connection to end --
// and the program then ends by itself with status 0 instead of dying of the signal. The signal
// is sent from inside the request handler, so it arrives while that request is in flight.
const http = require("node:http");

const server = http.createServer((request, response) => {
  console.log(`request ${request.url}`);
  process.kill(process.pid, "SIGTERM");
  setTimeout(() => {
    console.log("responding");
    response.end("done");
  }, 20);
});

process.on("SIGTERM", (name) => {
  console.log(`${name}: closing the server`);
  server.close(() => console.log("server closed after the last connection"));
});

server.listen(0, "127.0.0.1", () => {
  const request = http.get({ host: "127.0.0.1", port: server.address().port, path: "/work", agent: false }, (response) => {
    let body = "";
    response.setEncoding("utf8");
    response.on("data", (chunk) => { body += chunk; });
    response.on("end", () => console.log(`client got ${body}`));
  });
  request.on("error", (error) => console.log(`client error ${error.code}`));
});
