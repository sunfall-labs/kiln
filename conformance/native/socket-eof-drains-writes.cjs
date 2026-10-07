const net = require("node:net");
let reply = "";
let writeDone = false;
let serverSocketClosed = false;
const server = net.createServer(socket => {
  let request = "";
  socket.setEncoding("utf8");
  socket.on("data", data => request += data);
  socket.on("end", () => {
    if (request !== "request") throw new Error("wrong request before EOF");
    socket.write("reply", error => {
      if (error) throw error;
      writeDone = true;
    });
    // Default EOF handling must drain the final write before shutting down its writer.
  });
  socket.on("close", () => serverSocketClosed = true);
});
server.listen(0, "127.0.0.1", () => {
  const client = net.connect(server.address().port, "127.0.0.1");
  client.setEncoding("utf8");
  client.on("data", data => reply += data);
  client.on("connect", () => client.end("request"));
  client.on("close", () => {
    server.close(() => {
      if (reply !== "reply" || !writeDone || !serverSocketClosed) {
        throw new Error("EOF discarded a write or its successful callback");
      }
      console.log("EOF drains reply and closes both sockets");
    });
  });
});
