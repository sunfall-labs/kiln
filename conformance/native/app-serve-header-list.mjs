// `serve({ fetch })` with the header list as the one representation end to end: the engine hands
// a request's header block over in wire form (`\nname:value` per field), `Request.headers` reads
// it in place, a Response built from it keeps that same string, and `fetchRespond` hands it back
// for the engine to parse straight into Hyper's map. Raw sockets drive it, so the client sees
// exactly the fields on the wire -- duplicates, each Set-Cookie on its own line -- and can send
// what an ordinary client never does: no Host field (HTTP/1.0), an absolute-form target.
import { serve } from "node:http";
import net from "node:net";

const show = (value) => JSON.stringify(value);

const server = serve({
  port: 0,
  hostname: "127.0.0.1",
  async fetch(req) {
    const url = new URL(req.url);
    switch (url.pathname) {
      case "/echo":
        // What the handler sees, and the request's own headers sent straight back.
        return new Response(show({ method: req.method, url: req.url, headers: [...req.headers] }), {
          headers: req.headers,
        });
      case "/edit": {
        // A received request's headers are mutable; the edit is what goes back.
        const headers = req.headers;
        headers.delete("host");
        headers.set("x-dup", "replaced");
        headers.append("Set-Cookie", "a=1; Path=/");
        headers.append("set-cookie", "b=2");
        return new Response(null, { status: 204, headers });
      }
      case "/late": {
        // Read after an await: the list is the request's own string, not a borrowed buffer.
        await new Promise((resolve) => setTimeout(resolve, 5));
        return Response.json({ agent: req.headers.get("user-agent"), dup: req.headers.get("x-dup") });
      }
      case "/built": {
        const headers = new Headers([["x-b", "1"], ["X-B", "2"], ["content-type", "text/x-built"]]);
        const response = new Response("built", { headers, status: 207, statusText: "Multi" });
        headers.set("x-b", "after");
        response.headers.append("x-added", "later");
        return response;
      }
      default:
        return new Response(`url ${req.url}`);
    }
  },
});

await new Promise((resolve) => server.on("listening", resolve));
const port = server.address().port;

// One raw request; the answer's status line, its fields in arrival order (without `date`), and
// its body.
function raw(text) {
  return new Promise((resolve, reject) => {
    let got = "";
    const socket = net.connect(port, "127.0.0.1", () => socket.write(text));
    socket.setEncoding("utf8");
    socket.on("data", (data) => {
      got += data;
      const end = got.indexOf("\r\n\r\n");
      if (end < 0) return;
      const length = /\r\ncontent-length: (\d+)/i.exec(got.slice(0, end));
      if (length === null || got.length >= end + 4 + Number(length[1])) socket.end();
    });
    socket.on("close", () => {
      const end = got.indexOf("\r\n\r\n");
      const [status, ...fields] = got.slice(0, end).split("\r\n");
      resolve([status, fields.filter((field) => !/^date:/i.test(field)), got.slice(end + 4)]);
    });
    socket.on("error", reject);
  });
}

const echo = await raw(
  "GET /echo?q=1 HTTP/1.1\r\nHost: example.test:81\r\nX-Dup: one\r\nx-dup: two\r\n" +
    "Accept: */*\r\nX-Colon: a:b:c\r\nX-Empty:\r\nConnection: close\r\n\r\n"
);
console.log(echo[0]);
console.log(show(echo[1]));
console.log(echo[2]);

const edit = await raw("GET /edit HTTP/1.1\r\nHost: h.test\r\nX-Dup: 1\r\nConnection: close\r\n\r\n");
console.log(edit[0], show(edit[1]), show(edit[2]));

const late = await raw("GET /late HTTP/1.1\r\nHost: h.test\r\nUser-Agent: probe/1\r\nX-Dup: a\r\nX-Dup: b\r\nConnection: close\r\n\r\n");
console.log(late[0], show(late[1]), late[2]);

const built = await raw("GET /built HTTP/1.1\r\nHost: h.test\r\nConnection: close\r\n\r\n");
console.log(built[0], show(built[1]), built[2]);

// No Host field: the URL names the address the server listens on.
const http10 = await raw("GET /plain?x=1 HTTP/1.0\r\n\r\n");
console.log(http10[0], http10[2] === `url http://127.0.0.1:${port}/plain?x=1`, http10[2].replace(String(port), "PORT"));

// An absolute-form target is the URL, whatever Host says.
const absolute = await raw("GET http://proxy.test:8080/abs?y=2 HTTP/1.1\r\nHost: other.test\r\nConnection: close\r\n\r\n");
console.log(absolute[0], absolute[2]);

// An empty Host field counts as none.
const empty = await raw("GET /e HTTP/1.1\r\nHost:\r\nConnection: close\r\n\r\n");
console.log(empty[0], empty[2].replace(String(port), "PORT"));

server.close();
console.log("closed");
