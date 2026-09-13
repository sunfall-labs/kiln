// What `require("node:http2")` publishes, and what functions answer.
const http2 = require("node:http2");

const names = Object.getOwnPropertyNames(http2).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof http2[name]);
}

console.log("default self-link:", http2.default === http2);

let threwCreate = false;
try {
  http2.createServer();
} catch (e) {
  threwCreate = e.message.includes("requires an HTTP/2 host capability");
}
console.log("createServer threw:", threwCreate);

let threwSecure = false;
try {
  http2.createSecureServer();
} catch (e) {
  threwSecure = e.message.includes("requires an HTTP/2 host capability");
}
console.log("createSecureServer threw:", threwSecure);
