import http from "node:http";

const routes = new Map([
  ["/", () => ({ ok: true })],
  ["/echo", (url) => ({ path: url.pathname, q: url.searchParams.get("q") })],
]);

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, "http://127.0.0.1");
  const handler = routes.get(url.pathname);
  const body = handler ? await handler(url) : { error: "not found" };
  res.writeHead(handler ? 200 : 404, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
});

function get(port, path) {
  return new Promise((resolve) => {
    http.get({ host: "127.0.0.1", port, path }, (res) => {
      let body = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => { body += chunk; });
      res.on("end", () => resolve([res.statusCode, body]));
    });
  });
}

server.listen(0, "127.0.0.1", async () => {
  const { port } = server.address();
  for (const path of ["/", "/echo?q=1", "/missing"]) {
    const [status, body] = await get(port, path);
    console.log(status, body);
  }
  server.close();
});
