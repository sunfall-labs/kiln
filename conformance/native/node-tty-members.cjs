// What `require("node:tty")` publishes, and what functions answer.
const tty = require("node:tty");

const names = Object.getOwnPropertyNames(tty).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof tty[name]);
}

console.log("default self-link:", tty.default === tty);
console.log("isatty is fn:", typeof tty.isatty === "function");
console.log("ReadStream is fn:", typeof tty.ReadStream === "function");
console.log("WriteStream is fn:", typeof tty.WriteStream === "function");

console.log("isatty(-1):", tty.isatty(-1));
console.log("isatty('string'):", tty.isatty("string"));
console.log("isatty(null):", tty.isatty(null));
console.log("isatty(undefined):", tty.isatty(undefined));

const rs = new tty.ReadStream(0);
console.log("rs is obj:", typeof rs === "object");
console.log("rs.isTTY is bool:", typeof rs.isTTY === "boolean");
console.log("rs.isRaw is bool:", typeof rs.isRaw === "boolean");
console.log("rs.fd:", rs.fd);
rs.setRawMode(true);
console.log("rs.isRaw after setRawMode:", rs.isRaw);

const ws = new tty.WriteStream(1);
console.log("ws is obj:", typeof ws === "object");
console.log("ws.isTTY is bool:", typeof ws.isTTY === "boolean");
console.log("ws.fd:", ws.fd);
console.log("ws.columns is num:", typeof ws.columns === "number");
console.log("ws.rows is num:", typeof ws.rows === "number");
console.log("ws.getColorDepth is fn:", typeof ws.getColorDepth === "function");
console.log("ws.hasColors is fn:", typeof ws.hasColors === "function");
console.log("ws.getWindowSize is fn:", typeof ws.getWindowSize === "function");
