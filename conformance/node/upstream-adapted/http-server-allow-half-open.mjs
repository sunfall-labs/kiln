import http from "node:http";
import { connect } from "node:net";

const server = http.createServer((request, response) => {
  setTimeout(() => response.end("half-open"), 1);
});
server.httpAllowHalfOpen = true;

server.listen(0, "127.0.0.1", () => {
  const socket = connect(server.address().port, "127.0.0.1", () => {
    socket.end("GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
  });
  let bytes = "";
  socket.setEncoding("utf8");
  socket.on("data", chunk => bytes += chunk);
  socket.on("end", () => {
    if (!bytes.includes("half-open")) throw new Error("response was lost after client half-close");
    console.log("pass");
    server.close();
  });
});
