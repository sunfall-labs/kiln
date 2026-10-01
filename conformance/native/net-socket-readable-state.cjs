const net = require("node:net");

function assert(value, message) {
  if (!value) throw new Error(message);
}

const server = net.createServer((socket) => {
  socket.end("payload");
});
server.listen(0, "127.0.0.1", () => {
  const chunks = [];
  const socket = net.connect(server.address().port, "127.0.0.1");
  socket.on("data", (chunk) => chunks.push(chunk.toString()));
  socket.on("end", () => {
    assert(socket._readableState.endEmitted === true, "socket readable state records end emission");
    assert(socket._readableState.length === 0, "flowing socket has no buffered bytes after end");
    assert(socket.read() === null, "read after consumed end returns null");
    assert(chunks.join("") === "payload", "data arrived before the end event");
    socket.destroy();
    server.close(() => console.log("net.Socket readable state tracks end and read"));
  });
});
