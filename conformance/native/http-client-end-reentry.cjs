const http = require("node:http");

function assert(value, message) {
  if (!value) throw new Error(message);
}

let acceptedConnections = 0;
let requestCount = 0;
const server = http.createServer((request, response) => {
  requestCount += 1;
  response.end(requestCount === 1 ? "first" : "nested");
});
server.on("connection", () => {
  acceptedConnections += 1;
});

const timeout = setTimeout(() => {
  throw new Error("same-origin request from response end did not complete");
}, 8000);

server.listen(0, "127.0.0.1", () => {
  const url = `http://127.0.0.1:${server.address().port}/`;
  const firstRequest = http.get(url, (firstResponse) => {
    const firstSocket = firstResponse.socket;
    assert(firstSocket !== null, "the first response exposes its active socket");
    firstResponse.resume();
    firstResponse.on("end", () => {
      assert(firstResponse.socket === null, "reusable response detaches its socket before end listeners");

      const nestedRequest = http.get(url, (nestedResponse) => {
        assert(nestedResponse.socket !== firstSocket, "reentrant request uses a distinct connection");
        nestedResponse.resume();
        nestedResponse.on("end", () => {
          assert(requestCount === 2, "both same-origin requests reached the server");
          assert(acceptedConnections === 2, "the reentrant request opened a second connection");
          clearTimeout(timeout);
          server.close(() => console.log("HTTP response end reentry keeps Agent ownership ordered"));
        });
      });
      nestedRequest.on("error", (error) => {
        throw error;
      });
      assert(nestedRequest.socket === null, "new request has no socket before assignment");

      firstSocket.destroy();
    });
  });
  firstRequest.on("error", (error) => {
    throw error;
  });
});
