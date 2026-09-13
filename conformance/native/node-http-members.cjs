// What `require("node:http")` publishes, and what functions answer.
const http = require("node:http");

const names = Object.getOwnPropertyNames(http).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof http[name]);
}

console.log("default self-link:", http.default === http);
console.log("METHODS is array:", Array.isArray(http.METHODS));
console.log("METHODS length:", http.METHODS.length);
console.log("METHODS has GET:", http.METHODS.includes("GET"));
console.log("METHODS has POST:", http.METHODS.includes("POST"));

console.log("Server is callable:", typeof http.Server === "function");
console.log("createServer is callable:", typeof http.createServer === "function");
console.log("Server !== createServer:", http.Server !== http.createServer);
console.log("Server.name:", http.Server.name);
console.log("createServer.name:", http.createServer.name);

const s1 = http.createServer(() => {});
console.log("s1 type:", typeof s1);
console.log("s1 has listen:", typeof s1.listen === "function");
console.log("s1 has close:", typeof s1.close === "function");
s1.close();

const s2 = new http.Server(() => {});
console.log("s2 type:", typeof s2);
console.log("s2 has listen:", typeof s2.listen === "function");
console.log("s2 has close:", typeof s2.close === "function");
s2.close();
