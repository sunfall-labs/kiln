const net = require("node:net");

let request = "";
let reply = "";
let writeDone = false;
let serverSocketClosed = false;
let serverCloseCallback = false;

const server = net.createServer(socket => {
  socket.setEncoding("utf8");
  socket.on("data", data => request += data);
  socket.on("end", () => {
    if (request !== "request") throw new Error("server close discarded the accepted request");
    socket.write("reply", error => {
      if (error) throw error;
      writeDone = true;
    });
  });
  socket.on("close", () => serverSocketClosed = true);

  // Closing from the connection callback stops new accepts, but must let this accepted socket
  // finish its readable side and drain the reply before the server's close callback runs.
  server.close(() => {
    if (!serverSocketClosed) throw new Error("server close callback preceded socket close");
    serverCloseCallback = true;
  });
});

server.listen(0, "127.0.0.1", () => {
  const client = net.connect(server.address().port, "127.0.0.1");
  client.setEncoding("utf8");
  client.on("data", data => reply += data);
  client.on("connect", () => client.end("request"));
  client.on("close", () => {
    if (request !== "request" || reply !== "reply" || !writeDone ||
        !serverSocketClosed || !serverCloseCallback) {
      throw new Error("server close did not drain the accepted connection");
    }
    console.log("server close drains accepted connection");
  });
});
