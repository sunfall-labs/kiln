// Tier B server (needs node:http): runs on node, bun, kiln (+ --node).
// Porffor and Hermes are capability-gated out.
//
// Two routes: /hello (fixed text) and /sum?a=&b= (JSON). The port is a
// constant, not argv: not every backend here parses argv, and hard-coding it
// keeps the program inside the surface they share (same rule as
// bench/server/serve.mjs). Runs are sequential; the driver waits for the
// port to close between runtimes.
import http from "node:http";

const PORT = 18091;

const server = http.createServer((req, res) => {
  const url = req.url ?? "/";
  if (url === "/hello") {
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("hello from realworld");
  } else if (url.startsWith("/sum")) {
    const q = new URL(url, "http://127.0.0.1").searchParams;
    const a = Number(q.get("a") ?? "0");
    const b = Number(q.get("b") ?? "0");
    const body = JSON.stringify({ sum: a + b });
    res.writeHead(200, { "content-type": "application/json" });
    res.end(body);
  } else {
    res.writeHead(404, { "content-type": "text/plain" });
    res.end("not found");
  }
});

server.listen(PORT, "127.0.0.1", () => {
  console.log("listening");
});
