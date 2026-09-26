// The lifetime of the `req` and `res` a `node:http` server hands its listener, on a keep-alive
// connection where one socket carries every request.
//
// The native backend used to recycle one request/response pair per socket and reset it between
// requests: a request a program kept changed its `url` when the next one arrived, members a
// handler added (`req.user`, `res.locals`) leaked into the next request, and `req.headers` read
// after the listener returned could see the next request's header bytes, because the runtime
// reuses that buffer. `res.statusCode = 404` was also ignored in favour of the last `writeHead`.
// Everything below is read either late (after a timer, after later requests) or from a kept
// object, so each of those shows up as a divergence rather than as a coincidence of timing.
import http from "node:http";
import net from "node:net";

const kept = [];
const lines = [];
const server = http.createServer(async (req, res) => {
  const seen = { url: req.url, method: req.method, req, res };
  kept.push(seen);
  lines.push(`fresh ${req.url} ${req.tag} ${res.locals} ${Object.keys(req).includes("tag")}`);
  req.tag = `tag${kept.length}`;
  res.locals = { n: kept.length };
  // The listener returns here; everything after reads a request whose head dispatch is over.
  await new Promise((resolve) => setTimeout(resolve, 1));
  const headers = req.headers;
  lines.push(`late ${req.url} ${JSON.stringify(headers)} ${headers === req.headers}`);
  switch (req.url) {
    case "/status": {
      res.statusCode = 404;
      res.setHeader("X-Kind", "status");
      res.end("missing");
      break;
    }
    case "/headers": {
      res.setHeader("X-One", "1");
      res.setHeader("x-one", "2");
      res.setHeader("Content-Type", "text/plain");
      lines.push(`get ${res.getHeader("X-ONE")} ${res.getHeader("content-type")} ${res.hasHeader("nope")} ${res.hasHeader("x-One")}`);
      const all = res.getHeaders();
      lines.push(`getHeaders ${JSON.stringify(all)} ${Object.getPrototypeOf(all)}`);
      res.removeHeader("X-ONE");
      lines.push(`removed ${res.getHeader("x-one")} ${JSON.stringify(res.getHeaders())}`);
      res.writeHead(202, { "X-Two": 2, "content-type": "text/html" });
      lines.push(`writeHead ${res.statusCode} ${res.getHeader("CONTENT-TYPE")} ${res.getHeader("x-two")}`);
      res.end("héllo ✓");
      lines.push(`ended ${res.writableEnded}`);
      break;
    }
    case "/delete": {
      delete res.statusCode;
      delete req.method;
      lines.push(`deleted ${res.statusCode} ${req.method}`);
      res.end("deleted");
      break;
    }
    case "/assign": {
      req.headers = { replaced: "yes" };
      lines.push(`assigned ${JSON.stringify(req.headers)}`);
      res.writeHead(200);
      res.end();
      break;
    }
    case "/bad": {
      try {
        res.writeHead(42);
      } catch (error) {
        lines.push(`writeHead(42) ${error.name}`);
      }
      res.end("bad");
      break;
    }
    default:
      res.end(`ok ${req.url}`);
  }
});

await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const port = server.address().port;

const get = (path, headers = {}) =>
  new Promise((resolve, reject) => {
    const request = http.get({ host: "127.0.0.1", port, path, headers }, (response) => {
      let body = "";
      response.setEncoding("utf8");
      response.on("data", (chunk) => (body += chunk));
      response.on("end", () =>
        resolve(
          [response.statusCode, response.headers["content-type"] ?? "-", response.headers["x-kind"] ?? "-", body].join(" "),
        ),
      );
    });
    request.on("error", reject);
  });

// One raw request, so the head carries exactly the bytes written: repeated fields, a repeated
// `Set-Cookie`, mixed-case names, arrival order that is not name order.
const raw = (text) =>
  new Promise((resolve, reject) => {
    const socket = net.connect(port, "127.0.0.1", () => socket.write(text));
    let got = "";
    socket.on("data", (data) => (got += data));
    socket.on("end", () => resolve(got.split("\r\n")[0]));
    socket.on("close", () => resolve(got.split("\r\n")[0]));
    socket.on("error", reject);
  });

lines.push(`response ${await get("/first", { "X-Probe": "one" })}`);
lines.push(`response ${await get("/status", { "X-Probe": "two" })}`);
lines.push(`response ${await get("/headers", { "X-Probe": "three", "X-Case": "MiXeD" })}`);
lines.push(`response ${await get("/delete", { "X-Probe": "four" })}`);
lines.push(`response ${await get("/assign", { "X-Probe": "five" })}`);
lines.push(`response ${await get("/bad", { "X-Probe": "six" })}`);
lines.push(
  `raw ${await raw(
    "GET /raw?q=1 HTTP/1.1\r\nHost: h\r\nX-Dup: a\r\nZeta: z\r\nx-dup: b\r\nSet-Cookie: c=1\r\nSet-Cookie: d=2\r\nAlpha: A\r\nConnection: close\r\n\r\n",
  )}`,
);
await new Promise((resolve) => setTimeout(resolve, 5));

// Every request, read again after every later one has come and gone on the same connection.
for (const { url, method, req, res } of kept) {
  const headers = { ...req.headers };
  delete headers.host;
  lines.push(
    `kept ${url} ${req.url === url} ${method} ${req.tag} ${res.locals.n} ${res.req === req} ${JSON.stringify(headers)}`,
  );
}
lines.push(`distinct ${new Set(kept.map((entry) => entry.req)).size} ${new Set(kept.map((entry) => entry.res)).size}`);

server.close();
for (const line of lines) {
  // The host header carries the ephemeral port, which is not the program's answer.
  console.log(line.replace(/"host":"[^"]*",?/g, "").replace(/,}/g, "}"));
}
