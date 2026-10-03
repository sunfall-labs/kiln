let otherOrigin = "";
const target = Bun.serve({
  port: 0,
  hostname: "127.0.0.1",
  fetch(request) {
    return Response.json({
      authorization: request.headers.get("authorization"),
      cookie: request.headers.get("cookie"),
    });
  },
});
if (typeof target.on === "function") await new Promise((resolve) => target.on("listening", resolve));
const targetAddress = typeof target.address === "function" ? target.address() : target.address;
otherOrigin = `http://127.0.0.1:${target.port || targetAddress.port}`;

const server = Bun.serve({
  port: 0,
  hostname: "127.0.0.1",
  async fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/cross-origin") {
      return new Response(null, { status: 302, headers: { location: `${otherOrigin}/final` } });
    }
    if (url.pathname === "/loop") {
      return new Response(null, { status: 302, headers: { location: "/loop" } });
    }
    if (url.pathname === "/final") {
      const body = await request.text();
      return Response.json({
        method: request.method,
        body,
        authorization: request.headers.get("authorization"),
        contentType: request.headers.get("content-type"),
        contentLength: request.headers.get("content-length"),
      });
    }
    const status = Number(url.searchParams.get("status"));
    return new Response(null, { status, headers: { location: "../final" } });
  },
});

if (typeof server.on === "function") await new Promise((resolve) => server.on("listening", resolve));
const address = typeof server.address === "function" ? server.address() : server.address;
const base = `http://127.0.0.1:${server.port || address.port}`;
for (const status of [301, 302, 303, 307, 308]) {
  const response = await fetch(`${base}/redirect?status=${status}`, {
    method: "POST",
    headers: { authorization: "secret", "content-type": "application/custom" },
    body: "payload",
  });
  const value = await response.json();
  console.log(status, response.status, response.redirected, new URL(response.url).pathname, value.method, value.body, value.authorization, value.contentType, value.contentLength);
}
const unicode = await fetch(`${base}/final`, { method: "POST", body: "é😀" });
const unicodeValue = await unicode.json();
console.log("unicode-length", unicodeValue.body, unicodeValue.contentLength);
for (const method of ["GET", "HEAD"]) {
  const response = await fetch(`${base}/redirect?status=303`, { method });
  console.log("303", method, response.redirected, new URL(response.url).pathname);
}
const crossOrigin = await fetch(`${base}/cross-origin`, { headers: { authorization: "secret", cookie: "session=secret" } });
const crossOriginValue = await crossOrigin.json();
console.log("cross-origin", crossOrigin.status, crossOriginValue.authorization, crossOriginValue.cookie);
const manual = await fetch(`${base}/redirect?status=302`, { redirect: "manual" });
console.log("manual", manual.status, manual.headers.get("location"));
try {
  await fetch(`${base}/redirect?status=302`, { redirect: "error" });
} catch (error) {
  console.log("error", error instanceof TypeError);
}
try {
  await fetch(`${base}/loop`);
} catch (error) {
  console.log("limit", error instanceof TypeError);
}
if (typeof server.stop === "function") server.stop();
else server.close();
if (typeof target.stop === "function") target.stop();
else target.close();
