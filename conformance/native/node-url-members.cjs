// What `require("node:url")` publishes, and what functions answer.
const url = require("node:url");

const names = Object.getOwnPropertyNames(url).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof url[name]);
}

console.log("default self-link", url.default === url);
console.log("URL is fn:", typeof url.URL === "function");
console.log("URLSearchParams is fn:", typeof url.URLSearchParams === "function");

// fileURLToPath
console.log("f2p str:", url.fileURLToPath("file:///etc/hosts"));
console.log("f2p localhost:", url.fileURLToPath("file://localhost/etc/hosts"));
console.log("f2p encoded:", url.fileURLToPath("file:///path%20with%20spaces/test.txt"));
const urlObj = new url.URL("file:///home/user/code.js");
console.log("f2p URL obj:", url.fileURLToPath(urlObj));

// pathToFileURL
const fileUrl = url.pathToFileURL("/var/log/syslog");
console.log("p2f proto:", fileUrl.protocol);
console.log("p2f path:", fileUrl.pathname);

// parse
const parsed1 = url.parse("http://user:pass@example.com:8080/p/a/t/h?q=1&repeat=a&repeat=b#section", true);
console.log("p1 proto:", parsed1.protocol);
console.log("p1 auth:", parsed1.auth);
console.log("p1 host:", parsed1.host);
console.log("p1 hostname:", parsed1.hostname);
console.log("p1 port:", parsed1.port);
console.log("p1 pathname:", parsed1.pathname);
console.log("p1 search:", parsed1.search);
console.log("p1 hash:", parsed1.hash);
console.log("p1 query.q:", parsed1.query.q);
console.log("p1 query.repeat:", parsed1.query.repeat.join(","));

const parsedRel = url.parse("/relative/path?x=hello", false);
console.log("pRel proto:", parsedRel.protocol);
console.log("pRel pathname:", parsedRel.pathname);
console.log("pRel query:", parsedRel.query);

// format
const formatted = url.format({
  protocol: "http:",
  hostname: "example.org",
  port: 80,
  pathname: "/api",
  query: { page: 1 },
  hash: "#data",
});
console.log("fmt:", formatted);

// urlToHttpOptions
const httpOpts = url.urlToHttpOptions(new url.URL("http://alice:secret@example.com:9000/endpoint?check=1"));
console.log("opts proto:", httpOpts.protocol);
console.log("opts hostname:", httpOpts.hostname);
console.log("opts port:", httpOpts.port);
console.log("opts auth:", httpOpts.auth);
console.log("opts path:", httpOpts.path);

// domainToASCII and domainToUnicode
console.log("d2a:", url.domainToASCII("example.com"));
console.log("d2u:", url.domainToUnicode("example.com"));
