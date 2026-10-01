const net = require("node:net");

function assert(value, message) {
  if (!value) throw new Error(message);
}

let accepted;
const server = net.createServer((socket) => {
  accepted = socket;
  socket.on("data", (chunk) => {
    const text = chunk.toString();
    if (text === "ack1") socket.write("d");
    else if (text === "ack2") socket.write("e");
    else if (text === "ack3") socket.end();
  });
  socket.write("abc");
});

server.listen(0, "127.0.0.1", () => {
  const socket = net.connect(server.address().port, "127.0.0.1");
  socket.pause();
  let stage = 0;
  socket.on("readable", () => {
    if (stage === 0) {
      assert(socket.read(5) === null, "oversized read waits while the socket is still open");
      assert(socket._readableState.length === 3, "oversized read retains all buffered bytes");
      assert(socket.read(1).toString() === "a", "partial read consumes exactly one byte");
      assert(socket.read(5) === null, "oversized partial read still waits for more data");
      assert(socket._readableState.length === 2, "partial remainder stays buffered");
      stage = 1;
      socket.write("ack1");
      return;
    }
    if (stage === 1) {
      assert(socket.read(1).toString() === "b", "bounded read consumes the requested byte");
      stage = 2;
      socket.write("ack2");
      return;
    }
    if (stage === 2) {
      let available = "";
      let chunk;
      while ((chunk = socket.read()) !== null) available += chunk.toString();
      assert(available === "cde", `unbounded read drains available bytes before EOF: ${available}`);
      stage = 3;
      socket.write("ack3");
    }
  });
  socket.on("end", () => {
    assert(stage === 3, "end waits until buffered bytes have been consumed");
    assert(socket._readableState.endEmitted === true, "readable state records end emission");
    assert(socket._readableState.length === 0, "readable state has no bytes after end");
    assert(socket.read() === null, "read after consumed end returns null");
    socket.destroy();
    accepted.destroy();
    server.close(() => console.log("net.Socket readable reads retain bytes and defer end"));
  });
});
