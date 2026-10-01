// Socket EOF policy belongs to the constructor: raw net servers project their option onto each
// accepted socket, while HTTP keeps its own parser policy and exposes half-open sockets.
const net = require("node:net");
const http = require("node:http");

function assert(value, message) {
  if (!value) throw new Error(message);
}

const truthyPolicy = net.createServer({ allowHalfOpen: "yes" });
assert(truthyPolicy.allowHalfOpen === "yes", "server preserves a truthy option value");
function checkNetServer(options, expected, done, serverValue = expected) {
  const listener = (socket) => {
    assert(socket.allowHalfOpen === expected, `net accepted allowHalfOpen ${socket.allowHalfOpen}`);
    socket.destroy();
    server.close(done);
  };
  const server = options === undefined ? net.createServer(listener) : net.createServer(options, listener);
  assert(server.allowHalfOpen === (options === undefined ? false : true), `net server allowHalfOpen ${server.allowHalfOpen}`);
  server.allowHalfOpen = serverValue;
  server.listen(0, "127.0.0.1", () => {
    const client = net.connect(server.address().port, "127.0.0.1");
    assert(client.allowHalfOpen === false, "outgoing net socket defaults to false");
    client.on("error", () => {});
  });
}

checkNetServer(undefined, false, () => {
  checkNetServer({ allowHalfOpen: true }, true, () => {
    checkNetServer({ allowHalfOpen: true }, false, () => {
      const server = http.createServer({ allowHalfOpen: false }, (request, response) => {
        assert(server.allowHalfOpen === true, "HTTP server public allowHalfOpen stays true");
        assert(request.socket.allowHalfOpen === true, "HTTP accepted socket stays half-open");
        response.end("ok");
      });
      server.listen(0, "127.0.0.1", () => {
        const request = http.get({ host: "127.0.0.1", port: server.address().port }, (response) => {
          assert(response.socket.allowHalfOpen === false, "outgoing HTTP socket defaults to false");
          response.resume();
          response.on("end", () => server.close(() => console.log("socket half-open defaults and options: ok")));
        });
        request.on("error", (error) => { throw error; });
      });
    });
  });
});
