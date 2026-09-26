// `Headers`, and the header lists `Request` and `Response` keep, against the Fetch standard. A
// header list is one string in wire form (`\nname:value` per field) in both engines, so every
// operation here -- lookup, the edits, the sorted-and-combined iteration, the validation that
// keeps the form unambiguous, and the copies an init makes -- is a string operation that must
// answer exactly what node's Headers answers. Every line printed here was checked against node.
const show = (value) => JSON.stringify(value);
const attempt = (label, run) => {
  try {
    console.log(label, show(run()));
  } catch (error) {
    console.log(label, error.constructor.name);
  }
};

// Names are case-insensitive; a repeated name's values join with ", " in the order they came.
const h = new Headers({ "Content-Type": "text/html", "X-Dup": "1" });
h.append("x-dup", "2");
h.append("X-DUP", "3");
console.log(show([h.get("content-type"), h.get("CONTENT-TYPE"), h.get("x-dup"), h.has("X-Dup"), h.has("x-none"), h.get("x-none")]));

// `set` replaces every field of the name; `delete` removes every one; both leave the rest.
h.set("X-Dup", "only");
console.log(show([...h]));
h.delete("content-type");
h.append("content-type", "a/b");
console.log(show([...h]), h.get("content-type"));

// A name that is the tail of another, or appears inside a value, is not that field.
const tails = new Headers([["x-host", "a"], ["forwarded", "host=b"], ["host", "c:1"], ["ost", "d"]]);
console.log(show([tails.get("host"), tails.get("ost"), tails.get("x-host"), tails.get("st")]));

// Set-Cookie: `get` joins, `getSetCookie` does not, and iteration keeps each its own pair.
const cookies = new Headers();
cookies.append("Set-Cookie", "a=1; Path=/");
cookies.append("x-other", "o");
cookies.append("set-cookie", "b=2, c=3");
console.log(show(cookies.get("set-cookie")), show(cookies.getSetCookie()), show([...cookies]));
console.log(show(new Headers().getSetCookie()));

// Iteration is sorted by name (code unit order) and combined, whatever the insertion order.
const order = new Headers([["b", "1"], ["A", "2"], ["_c", "3"], ["b", "4"], ["a-", "5"], ["0", "6"]]);
console.log(show([...order.keys()]), show([...order.values()]), show([...order.entries()]));
const seen = [];
order.forEach(function (value, name, parent) {
  seen.push([name, value, parent === order, this.tag]);
}, { tag: "this" });
console.log(show(seen));
attempt("forEach(non-function)", () => order.forEach(1));

// Values: HTTP whitespace (SP, HTAB, CR, LF) is stripped from both ends and kept inside.
const ws = new Headers();
ws.append("x-a", " \t padded \t ");
ws.append("x-b", "in  side\tkept");
ws.append("x-c", "\r\n\t ");
ws.append("x-d", " nbspÿ");
console.log(show([...ws]));

// Invalid names and values are TypeErrors, from every entry point.
for (const name of ["", "a b", "a:b", "a\nb", "é", "[x]", "a\u0000"]) attempt(`name ${show(name)}`, () => new Headers([[name, "v"]]).get(name));
for (const value of ["a\nb", "a\rb", "a\u0000b", "Ā", "€"]) attempt(`value ${show(value)}`, () => new Headers({ "x-v": value }).get("x-v"));
attempt("get(bad)", () => new Headers().get("a b"));
attempt("has(bad)", () => new Headers().has("a:b"));
attempt("set(bad value)", () => new Headers().set("x", "a\nb"));
attempt("delete(bad)", () => new Headers().delete("a b"));
attempt("valid token chars", () => new Headers([["!#$%&'*+-.^_`|~09AZaz", "ok"]]).get("!#$%&'*+-.^_`|~09azaz"));

// Every init form: a record (own enumerable string keys, values stringified), a sequence of
// pairs, any iterable of iterables, a Map, another Headers.
console.log(show([...new Headers({ n: 1, u: undefined, z: null, b: true, o: { toString: () => "obj" } })]));
console.log(show([...new Headers([["a", "1"], ["a", "2"]])]));
console.log(show([...new Headers(new Map([["m", "1"], ["M", "2"]]))]));
console.log(show([...new Headers([new Set(["s", "v"])])]));
const proto = Object.create({ inherited: "no" });
proto.own = "yes";
console.log(show([...new Headers(proto)]));
attempt("pair of 1", () => new Headers([["a"]]));
attempt("pair of 3", () => new Headers([["a", "b", "c"]]));
attempt("pair not iterable", () => new Headers([1]));
attempt("string init", () => new Headers("a: b"));
attempt("null init", () => new Headers(null));
attempt("undefined init", () => [...new Headers(undefined)]);

// A Headers init is copied: changing the source afterwards changes nothing already made.
const source = new Headers({ "x-s": "1" });
const copy = new Headers(source);
source.set("x-s", "2");
source.append("x-t", "3");
console.log(show([...copy]), show([...source]));

// Response: the init's headers are copied once; the body's type is folded into the Response's
// own list only when the init names none, and never into the caller's Headers.
const given = new Headers({ "x-g": "g" });
const record = { "x-r": "r" };
const res = new Response("text", { headers: given, status: 201, statusText: "Made It" });
const res2 = new Response("text", { headers: record });
given.set("x-g", "changed");
record["x-r"] = "changed";
console.log(res.status, res.statusText, res.ok, show([...res.headers]), show([...given]), show([...res2.headers]));
console.log(res.headers === res.headers, res.headers instanceof Headers);
res.headers.set("x-late", "l");
console.log(show([...res.headers]));
console.log(new Response("x", { headers: { "Content-Type": "text/custom" } }).headers.get("content-type"));
console.log(new Response(new Uint8Array([1])).headers.get("content-type"), new Response(null).headers.get("content-type"), new Response().headers.get("content-type"));
console.log(new Response(new Blob(["b"], { type: "image/x-b" })).headers.get("content-type"), new Response(new Blob(["b"])).headers.get("content-type"));
console.log(new Response(new URLSearchParams("a=1&b=2")).headers.get("content-type"));
console.log(Response.json({ a: 1 }).headers.get("content-type"), Response.json(1, { headers: { "content-type": "text/json" }, status: 299 }).status);
console.log(Response.json(1, { headers: { "content-type": "text/json" } }).headers.get("content-type"));
console.log(Response.redirect("http://r.test/x", 308).headers.get("location"), Response.redirect("http://r.test/x").status);
attempt("status 199", () => new Response("", { status: 199 }));
attempt("status 600", () => new Response("", { status: 600 }));
attempt("status 204 with body", () => new Response("x", { status: 204 }));
attempt("status 204 null body", () => new Response(null, { status: 204 }).status);
attempt("status 65736 wraps", () => new Response(null, { status: 65736 }).status);
attempt("status '201'", () => new Response(null, { status: "201" }).status);
attempt("statusText newline", () => new Response(null, { statusText: "a\nb" }));
attempt("headers invalid", () => new Response(null, { headers: { "a b": "c" } }));
attempt("init not object", () => new Response(null, 5));
attempt("redirect 200", () => Response.redirect("http://x/", 200));
const cloned = res.clone();
res.headers.set("x-after-clone", "1");
console.log(cloned.status, cloned.statusText, show([...cloned.headers]), await cloned.text(), cloned.bodyUsed);
console.log(Response.error().type, Response.error().status, show([...Response.error().headers]));

// Request: headers from the init, or from the input Request (copied), with the body's type.
const req = new Request("http://q.test/a", { method: "post", headers: [["x-q", "1"]], body: "b" });
const req3 = new Request(req.clone());
const req2 = new Request(req, { headers: { "x-other": "o" } });
req.headers.set("x-q", "changed");
console.log(req.method, req.url, show([...req.headers]), show([...req2.headers]), show([...req3.headers]));
// Constructing from a Request takes its body: the input is used, and cannot be used again.
console.log(req.bodyUsed, req2.bodyUsed, await req2.text());
attempt("from a used Request", () => new Request(req));
attempt("from a used Request, new body", () => new Request(req, { body: "fresh" }).method);
console.log(new Request("http://q.test/", { method: "patch" }).method, new Request("http://q.test/", { method: "Delete" }).method);
attempt("GET with body", () => new Request("http://q.test/", { body: "x" }));
attempt("TRACE", () => new Request("http://q.test/", { method: "TRACE" }));
attempt("bad method", () => new Request("http://q.test/", { method: "a b" }));
console.log(await req3.text(), req3.bodyUsed);
attempt("clone after text", () => req3.clone());
const bodiless = new Request("http://q.test/b");
console.log(new Request(bodiless).bodyUsed, bodiless.bodyUsed);
