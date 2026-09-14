import http from "node:http";

const server = http.createServer((req, res) => {
  const chunks = [];
  req.on("data", (chunk) => { chunks.push(chunk); });
  req.on("end", () => {
    const body = Buffer.concat(chunks);
    res.writeHead(200, { "content-type": "application/octet-stream" });
    res.end(new Uint8Array([0, 255, 1, 10]));
  });
});

server.listen(0, "127.0.0.1", () => {
  const { port } = server.address();
  const request = http.request({ host: "127.0.0.1", port, method: "POST", path: "/binary" }, (res) => {
    const chunks = [];
    res.on("data", (chunk) => { chunks.push(chunk); });
    res.on("end", () => {
      const body = Buffer.concat(chunks);
      console.log(res.statusCode, body.toString("hex"));
      server.close();
    });
  });
  const source = Buffer.alloc(8);
  source[1] = 0;
  source[2] = 1;
  source[3] = 2;
  source[4] = 3;
  source[5] = 255;
  source[6] = 0;
  request.write(source.subarray(1, 4));
  request.end(source.subarray(4, 7));
});
