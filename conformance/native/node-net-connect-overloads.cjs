const net = require("node:net");

for (const port of [true, {}, Symbol("port")]) {
  try {
    net.connect({ port });
    console.log("unexpected port acceptance");
  } catch (error) {
    console.log(error.name);
  }
}

try {
  net.connect({ port: "not-a-port" });
  console.log("unexpected string port acceptance");
} catch (error) {
  console.log(error.name);
}

const server = net.createServer((socket) => socket.end());
server.listen(0, "127.0.0.1", () => {
  const port = server.address().port;
  // The two-argument overload must recognize the callback instead of stringifying it as host.
  const client = net.connect(port, () => {
    console.log("connected without explicit host");
    client.end();
    server.close();
  });
});
