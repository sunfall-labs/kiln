// What `require("node:net")` publishes, and what functions answer.
const net = require("node:net");

const names = Object.getOwnPropertyNames(net).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof net[name]);
}

console.log("default self-link:", net.default === net);

const server = net.createServer(() => {});
console.log("server type:", typeof server);
console.log("server has listen:", typeof server.listen === "function");
console.log("server has close:", typeof server.close === "function");
server.close();
