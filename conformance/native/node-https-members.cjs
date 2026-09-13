// What `require("node:https")` publishes, and what functions answer.
const https = require("node:https");

const names = Object.getOwnPropertyNames(https).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof https[name]);
}

console.log("default self-link:", https.default === https);

let threwCreate = false;
try {
  https.createServer();
} catch (e) {
  threwCreate = e.message.includes("requires a TLS host capability");
}
console.log("createServer threw:", threwCreate);
