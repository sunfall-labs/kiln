const net = require("node:net");

function assert(value, message) {
  if (!value) throw new Error(message);
}

let accepted;
const server = net.createServer((socket) => {
  accepted = socket;
  let commands = "";
  socket.on("data", (chunk) => {
    commands += chunk.toString();
    let newline;
    while ((newline = commands.indexOf("\n")) !== -1) {
      const command = commands.slice(0, newline);
      commands = commands.slice(newline + 1);
      if (command === "ready") socket.write("abc");
      else if (command === "ack1") socket.write("d");
      else if (command === "ack2") socket.write("e");
      else if (command === "ack3") socket.end();
    }
  });
});

server.listen(0, "127.0.0.1", () => {
  const socket = net.connect(server.address().port, "127.0.0.1");
  socket.pause();
  socket.once("connect", () => {
    assert(socket.read(1) === null, "an empty bounded read starts transport demand");
    assert(socket.read() === null, "an empty unbounded read starts transport demand");
    let rangeError;
    try {
      socket.read(1073741825);
    } catch (error) {
      rangeError = error;
    }
    assert(rangeError?.code === "ERR_OUT_OF_RANGE", "oversized read uses Node's coded range error");
    socket.write("ready\n");
  });
  let stage = 0;
  socket.on("readable", () => {
    if (stage === 0) {
      assert(socket.read(5) === null, "oversized read waits while the socket is still open");
      assert(socket._readableState.length === 3, "oversized read retains all buffered bytes");
      assert(socket.read(1).toString() === "a", "partial read consumes exactly one byte");
      assert(socket.read(5) === null, "oversized partial read still waits for more data");
      assert(socket._readableState.length === 2, "partial remainder stays buffered");
      stage = 1;
      socket.write("ack1\n");
      return;
    }
    if (stage === 1) {
      assert(socket.read(1).toString() === "b", "bounded read consumes the requested byte");
      stage = 2;
      socket.write("ack2\n");
      return;
    }
    if (stage === 2) {
      let available = "";
      let chunk;
      while ((chunk = socket.read()) !== null) available += chunk.toString();
      assert(available === "cde", `unbounded read drains available bytes before EOF: ${available}`);
      stage = 3;
      socket.write("ack3\n");
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
