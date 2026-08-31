import http from "node:http";

const server = http.createServer((req, res) => {
  const chunks = [];
  req.on("data", (chunk) => { chunks.push(chunk); });
  req.on("end", () => {
    const body = Buffer.concat(chunks);
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({
      method: req.method,
      bytes: body.length,
      text: body.toString("utf8"),
    }));
  });
});

server.listen(0, "127.0.0.1", () => {
  const { port } = server.address();
  const request = http.request({
    host: "127.0.0.1",
    port,
    method: "POST",
    path: "/upload",
    headers: { "content-type": "text/plain" },
  }, (res) => {
    let body = "";
    res.setEncoding("utf8");
    res.on("data", (chunk) => { body += chunk; });
    res.on("end", () => {
      console.log(res.statusCode, body);
      server.close();
    });
  });
  request.write("kiln ");
  request.write("streams");
  request.end();
});
