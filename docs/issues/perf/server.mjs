import http from "node:http";
const port = Number(process.argv[2] ?? process.env.PORT ?? 3000);
const server = http.createServer((req, res) => {
  res.writeHead(200, { "content-type": "text/plain" });
  res.end("kiln");
});
server.listen(port, "127.0.0.1", () => {
  console.log("listening", port);
});
