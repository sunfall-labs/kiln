// `URL`'s read surface, which both engines answer from the `url` crate: the interpreter links
// it directly, the native backend through the provider (issue full-runtime/04). Setters,
// iteration and `instanceof` are the interpreter's gap today (full-runtime/05) and are kept in
// `docs/issues/full-runtime/probes/url2.mjs`, where node is the reference.
const u = new URL("http://user:pw@example.com:8080/a/b?x=1&y=2#frag");
for (const k of ["href", "origin", "protocol", "username", "password", "host", "hostname", "port", "pathname", "search", "hash"]) {
  console.log(k, JSON.stringify(u[k]));
}
console.log("searchParams.get:", u.searchParams.get("y"), "| toString:", String(u), "| json:", JSON.stringify(u));
console.log(new URL("/x?y#z", "http://h:81/a/b").href, new URL("file:///tmp/a.txt").pathname, new URL("http://EXAMPLE.com:80/").host);
try { new URL("not a url"); } catch (e) { console.log(e.name, e.message.startsWith("Invalid URL")); }
