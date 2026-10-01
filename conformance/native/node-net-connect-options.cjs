// Connect options use ordinary property access. Inherited accessors must run before any socket
// is opened, and exceptions from them must remain observable.
const net = require("node:net");

const observed = [];
const options = Object.create({
  get port() {
    observed.push("inherited port");
    throw new Error("stop before connect");
  },
});

try {
  net.connect(options);
  console.log("unexpected socket");
} catch (error) {
  console.log(error.message);
}
console.log(observed.join(","));
