const u = new URL("https://example.org/base/path?a=1&b=two&a=3#top");
u.pathname = "/other"; u.hash = ""; u.port = "8443"; u.protocol = "http";
console.log(u.href, "|", u.origin, "|", u.port);
u.search = "?x=1"; console.log(u.search, u.searchParams.get("x"), [...u.searchParams].length);
u.searchParams.append("y", "a b&c"); u.searchParams.set("x", "9");
console.log(u.href, "|", String(u.searchParams), "|", u.searchParams.size);
u.searchParams.delete("y"); console.log(u.search);
console.log(Object.prototype.toString.call(u), Object.prototype.toString.call(u.searchParams), JSON.stringify({ u }));
console.log(URL.canParse("nope"), URL.parse("nope"), URL.canParse("/x", "http://h"), new URL("/x?y#z", "http://h:81/a/b").href);
try { new URL("not a url"); } catch (e) { console.log(e.name, e.message); }
console.log(new URL("file:///tmp/a.txt").pathname, new URL("http://EXAMPLE.com:80/").host, new URL("http://[::1]:3000/").hostname);
