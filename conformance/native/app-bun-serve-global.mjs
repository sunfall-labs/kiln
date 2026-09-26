// `Bun.serve` with nothing imported: the global is the only sign of the HTTP host a program
// gives, so the compiled backend must keep the HTTP dispatch groups for it (kiln-liveness) --
// before it did, the first request refused inside the bootstrap's `serve`. The client is
// `fetch`, also a global, so the program imports nothing at all.
const server = Bun.serve({
  port: 0,
  hostname: "127.0.0.1",
  fetch(req) {
    const url = new URL(req.url);
    if (req.method === "POST") {
      return req.text().then((body) => new Response(`${url.pathname} got ${body.length}`, { status: 201 }));
    }
    return Response.json({ path: url.pathname, query: url.searchParams.get("q"), agent: req.headers.has("host") });
  },
});

await new Promise((resolve) => server.on("listening", resolve));
const base = `http://127.0.0.1:${server.address().port}`;
const first = await fetch(`${base}/items?q=7`);
console.log(first.status, first.headers.get("content-type"), await first.text());
const second = await fetch(`${base}/upload`, { method: "POST", body: "abcdef" });
console.log(second.status, second.headers.get("content-type"), await second.text());
server.close();
console.log("closed");
