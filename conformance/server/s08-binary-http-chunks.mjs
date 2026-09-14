import http from "node:http";

const server = http.createServer((req, res) => {
  const chunks = [];
  req.on("data", (chunk) => { chunks.push(chunk); });
  req.on("end", () => {
    const body = Buffer.concat(chunks);
    const expected = new Uint8Array([0, 1, 2, 3, 255, 0]);
    let valid = body.length === expected.length;
    for (let i = 0; valid && i < expected.length; i += 1) valid = body[i] === expected[i];
    res.writeHead(valid ? 200 : 400, { "content-type": "application/octet-stream" });
    res.end(valid ? new Uint8Array([0, 255, 1, 10]) : new Uint8Array([255]));
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
  setTimeout(() => {
    request.write(Buffer.alloc(0));
    request.end(source.subarray(4, 7));
  }, 0);
});
