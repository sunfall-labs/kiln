import { createServer } from "node:http";
import { connect } from "node:net";

const server = createServer((request, response) => response.end("kiln utf8"));
server.listen(0, "127.0.0.1", () => {
  const socket = connect(server.address().port, "127.0.0.1", () => {
    if (socket.readyState !== "open") throw new Error("connected socket was not open");
    socket.end("GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    if (socket.readyState !== "readOnly") throw new Error("ended socket was not read-only");
  });
  let body = "";
  socket.setEncoding("utf8");
  socket.on("data", chunk => {
    if (typeof chunk !== "string") throw new Error("socket did not decode bytes");
    body += chunk;
  });
  socket.on("end", () => {
    if (!body.includes("kiln utf8")) throw new Error("decoded socket body mismatch");
    server.close();
  });
  socket.on("close", () => {
    if (socket.readyState !== "closed") throw new Error("closed socket retained an open half");
    console.log("pass");
  });
});
