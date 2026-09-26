// `serve({ fetch })` end to end, in one process: the handler sees a WHATWG Request built straight
// from the dispatched request (method, URL, lazily parsed headers, a whole body) and answers with
// a Response the host sends in one call. Every case prints what a client observed.
import http, { serve } from "node:http";
import { setTimeout as sleep } from "node:timers/promises";

const shared = new Headers({ "x-shared": "1" });

const server = serve({
  port: 0,
  hostname: "127.0.0.1",
  async fetch(req) {
    const url = new URL(req.url);
    switch (url.pathname) {
      case "/hello":
        return new Response(`hello ${req.method} ${url.search}`);
      case "/headers":
        return new Response(
          [req.headers.get("x-custom"), req.headers.has("x-missing"), req.headers.get("X-Custom"),
           req.url.startsWith("http://127.0.0.1")].join(" ")
        );
      case "/echo": {
        const text = await req.text();
        return Response.json({ method: req.method, length: text.length, text, type: req.headers.get("content-type") });
      }
      case "/json": {
        const value = await req.json();
        return Response.json({ sum: value.a + value.b }, { status: 201 });
      }
      case "/async":
        await sleep(2);
        return new Response("later", {
          status: 202,
          headers: [["x-one", "1"], ["set-cookie", "a=1"]],
        });
      case "/throw":
        throw new Error("boom");
      case "/reject":
        await sleep(1);
        throw new TypeError("late boom");
      case "/empty":
        return new Response(null, { status: 204 });
      case "/bytes":
        return new Response(new Uint8Array([104, 105]), { headers: { "content-type": "application/octet-stream" } });
      case "/shared":
        return new Response("shared", { headers: shared });
      case "/not-a-response":
        return "nope";
      case "/redirect":
        return Response.redirect("http://127.0.0.1/elsewhere", 307);
      default:
        return new Response("not found", { status: 404 });
    }
  },
});

await new Promise((resolve) => server.on("listening", resolve));
const port = server.address().port;

function request(method, path, { headers = {}, body } = {}) {
  return new Promise((resolve, reject) => {
    const req = http.request({ host: "127.0.0.1", port, method, path, headers }, (res) => {
      let data = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => (data += chunk));
      res.on("end", () => {
        const h = res.headers;
        resolve([res.statusCode, h["content-type"] ?? "-", h["content-length"] ?? "-", h["set-cookie"] ?? "-", h["x-one"] ?? h["x-shared"] ?? h.location ?? "-", data].join(" | "));
      });
    });
    req.on("error", reject);
    if (body !== undefined) req.write(body);
    req.end();
  });
}

console.log(await request("GET", "/hello?q=1"));
console.log(await request("GET", "/headers", { headers: { "x-custom": "abc" } }));
console.log(await request("POST", "/echo", { headers: { "content-type": "text/plain" }, body: "posted body" }));
console.log(await request("POST", "/json", { headers: { "content-type": "application/json" }, body: JSON.stringify({ a: 2, b: 3 }) }));
console.log(await request("PUT", "/echo", { body: "x".repeat(70000) }).then((line) => line.slice(0, 80)));
console.log(await request("GET", "/async"));
console.log(await request("GET", "/throw"));
console.log(await request("GET", "/reject"));
console.log(await request("GET", "/empty"));
console.log(await request("GET", "/bytes"));
console.log(await request("GET", "/shared"), shared.has("content-type"));
console.log(await request("GET", "/not-a-response"));
console.log(await request("GET", "/redirect"));
console.log(await request("HEAD", "/hello"));
console.log(await request("GET", "/missing"));

// Constructed Requests and Responses keep their semantics.
const constructed = new Request("http://example.test/a?b=1", { method: "post", body: "text", headers: { "X-A": " 1 " } });
console.log(constructed.method, constructed.url, constructed.headers.get("x-a"), constructed.headers.get("content-type"));
const response = new Response("body");
console.log(response.ok, response.status, response.headers.get("content-type"), await response.text());
console.log(new Response(null, { status: 500 }).ok, new Response(null).headers.get("content-type"));

server.close();
console.log("closed");
