import http from "node:http";
import { connect } from "node:net";

const server = http.createServer((request, response) => {
  setTimeout(() => response.end(request.url), request.url === "/first" ? 2 : 1);
});

server.listen(0, "127.0.0.1", () => {
  const socket = connect(server.address().port, "127.0.0.1", () => {
    socket.write(
      "GET /first HTTP/1.1\r\nHost: localhost\r\n\r\n" +
      "GET /second HTTP/1.1\r\nHost: localhost\r\n\r\n"
    );
  });
  let bytes = "";
  socket.setEncoding("utf8");
  socket.on("data", chunk => {
    bytes += chunk;
    if (bytes.includes("/first") && bytes.includes("/second")) socket.end();
  });
  socket.on("close", () => {
    // HTTP/1 may frame these bodies with Content-Length or chunked encoding. The upstream
    // invariant is request identity and response ordering, not a particular legal framing.
    const first = bytes.indexOf("/first");
    const second = bytes.indexOf("/second");
    if (first < 0 || second < 0 || first >= second) {
      throw new Error(`pipelined response order: ${first},${second},${bytes.length}`);
    }
    console.log("pass");
    socket.end();
    server.close();
  });
});
