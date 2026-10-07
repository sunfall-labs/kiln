const http = require("node:http");

function assert(value, message) {
  if (!value) throw new Error(message);
}

let connections = 0;
let closed = 0;
let requests = 0;
const server = http.createServer(() => {
  requests += 1;
});
server.on("connection", (socket) => {
  connections += 1;
  socket.once("close", () => {
    closed += 1;
  });
  socket.destroy();
});

const timeout = setTimeout(() => {
  throw new Error("destroyed accepted HTTP socket was not retired");
}, 5000);

server.listen(0, "127.0.0.1", () => {
  const request = http.get(`http://127.0.0.1:${server.address().port}/never`);
  request.on("error", () => {
    server.close(() => {
      assert(connections === 1, "the HTTP server published the accepted socket");
      assert(closed === 1, "destroying the socket emitted one close event");
      assert(requests === 0, "the HTTP parser did not dispatch a request after destroy");
      clearTimeout(timeout);
      console.log("HTTP connection listener can destroy an owned socket before request parsing");
    });
  });
});
