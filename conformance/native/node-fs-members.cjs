// What `require("node:fs")` publishes, and what functions answer.
const fs = require("node:fs");

const names = Object.getOwnPropertyNames(fs).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof fs[name]);
}

console.log("default self-link:", fs.default === fs);
console.log("promises is object:", typeof fs.promises === "object");
console.log("existsSync is function:", typeof fs.existsSync === "function");
console.log("promises.readFile is function:", typeof fs.promises.readFile === "function");
console.log("promises.writeFile is function:", typeof fs.promises.writeFile === "function");
console.log("promises.mkdir is function:", typeof fs.promises.mkdir === "function");
console.log("promises.stat is function:", typeof fs.promises.stat === "function");
console.log("promises.appendFile is function:", typeof fs.promises.appendFile === "function");
