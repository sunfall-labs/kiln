// A server request's body end reaches its Readable only once something reads the stream, so a
// request nobody streams never builds Readable state. Every way of reading must still see the
// end exactly once: an `end` listener alone, a listener and resume() added after the body has
// ended, `data` then `end` on an empty body, a POST body through `data`, and a request nobody
// reads at all. (An empty body read only after it ended, and async iteration, are left out
// while the interpreter's request is not a Readable --
// docs/issues/interpreter-incoming-message-readable.md.)
import http from "node:http";

const log = [];

const server = http.createServer((req, res) => {
  const path = req.url;
  if (path === "/end-only") {
    req.on("end", () => res.end(`end-only readableEnded=${req.readableEnded}`));
    req.resume();
  } else if (path === "/late") {
    setTimeout(() => {
      let ends = 0;
      req.on("end", () => {
        ends++;
        setTimeout(() => res.end(`late ends=${ends} readableEnded=${req.readableEnded}`), 5);
      });
      req.resume();
    }, 20);
  } else if (path === "/empty") {
    let chunks = 0;
    req.on("data", () => { chunks++; });
    req.on("end", () => res.end(`empty chunks=${chunks}`));
  } else if (path === "/post") {
    let body = "";
    req.setEncoding("utf8");
    req.on("data", (chunk) => { body += chunk; });
    req.on("end", () => res.end(`post body=${body}`));
  } else {
    res.end("unread");
  }
});

function request(method, path, body) {
  return new Promise((resolve, reject) => {
    const req = http.request({ host: "127.0.0.1", port: server.address().port, method, path },
      (res) => {
        let text = "";
        res.setEncoding("utf8");
        res.on("data", (chunk) => { text += chunk; });
        res.on("end", () => resolve(text));
      });
    req.on("error", reject);
    req.end(body);
  });
}

server.listen(0, "127.0.0.1", async () => {
  log.push(await request("GET", "/end-only"));
  log.push(await request("GET", "/late"));
  log.push(await request("GET", "/empty"));
  log.push(await request("POST", "/empty", ""));
  log.push(await request("POST", "/post", "posted"));
  log.push(await request("GET", "/unread"));
  log.push(await request("POST", "/unread", "ignored"));
  for (const line of log) console.log(line);
  server.close(() => console.log("closed"));
});
