import http from "node:http";

// Every form of listen() that takes a callback, and the 'listening' event the callback is a
// once-listener on. For each form the program records the order in which three things happen:
// the 'listening' event, the callback, and the first request the server serves. Node's order is
// event, callback, request -- the server is listening before anything can connect to it.
//
// The callback is the one thing a real server prints ("listening on ...") and the one thing a
// supervisor waits for, so a form that served requests without ever calling it was a server that
// looked dead. Each form runs alone and in sequence, on port 0.

const forms = [
  ["port,host,cb", (server, cb) => server.listen(0, "127.0.0.1", cb)],
  ["port,cb", (server, cb) => server.listen(0, cb)],
  ["port,host", (server) => server.listen(0, "127.0.0.1")],
  ["{port,host},cb", (server, cb) => server.listen({ port: 0, host: "127.0.0.1" }, cb)],
];

function runForm(label, start) {
  return new Promise((resolve) => {
    const order = [];
    const server = http.createServer((req, res) => {
      order.push("request");
      res.end("ok");
    });
    server.on("listening", () => order.push("event"));
    const cb = () => order.push("callback");
    start(server, cb);
    server.once("listening", () => {
      const { port } = server.address();
      http.get({ host: "127.0.0.1", port, path: "/" }, (res) => {
        res.resume();
        res.on("end", () => {
          server.close(() => {
            console.log(label, order.join(","));
            resolve();
          });
        });
      });
    });
  });
}

for (const [label, start] of forms) {
  await runForm(label, start);
}
