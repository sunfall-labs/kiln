import http from "node:http";

const routes = new Map([
  // The whole body arrives with `end`, so a length can be declared.
  ["/implicit", (req, res) => { res.end("kiln"); }],
  ["/implicit-empty", (req, res) => { res.end(); }],
  // A length is bytes, not code units: this body is five characters and six bytes.
  ["/implicit-utf8", (req, res) => { res.end("héllo"); }],
  // A framing the script chose is left alone in both directions.
  ["/declared-length", (req, res) => { res.setHeader("content-length", "4"); res.end("kiln"); }],
  ["/declared-chunked", (req, res) => { res.setHeader("transfer-encoding", "chunked"); res.end("kiln"); }],
  // A chunk before `end` leaves the length unknown.
  ["/streamed", (req, res) => { res.write("ki"); res.end("ln"); }],
  // `writeHead` commits the header block, so `end` can no longer declare a length.
  ["/write-head", (req, res) => { res.writeHead(200, { "content-type": "text/plain" }); res.end("kiln"); }],
  // A status that cannot carry a body carries no framing header either.
  ["/no-content", (req, res) => { res.writeHead(204); res.end(); }],
]);

const server = http.createServer((req, res) => {
  const handler = routes.get(req.url);
  if (handler) handler(req, res);
  else { res.writeHead(404); res.end(); }
});

function request(port, path, method) {
  return new Promise((resolve) => {
    const options = { host: "127.0.0.1", port, path, method };
    const client = http.request(options, (res) => {
      let body = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => { body += chunk; });
      res.on("end", () => resolve([
        res.statusCode,
        String(res.headers["content-length"]),
        String(res.headers["transfer-encoding"]),
        JSON.stringify(body),
      ]));
    });
    client.end();
  });
}

server.listen(0, "127.0.0.1", async () => {
  const { port } = server.address();
  const paths = [
    "/implicit",
    "/implicit-empty",
    "/implicit-utf8",
    "/declared-length",
    "/declared-chunked",
    "/streamed",
    "/write-head",
    "/no-content",
  ];
  for (const path of paths) {
    const [status, length, encoding, body] = await request(port, path, "GET");
    console.log("GET", path, status, length, encoding, body);
  }
  // A HEAD response never carries a body, and node declares no length for one.
  for (const path of ["/implicit", "/streamed"]) {
    const [status, length, encoding, body] = await request(port, path, "HEAD");
    console.log("HEAD", path, status, length, encoding, body);
  }
  server.close();
});
