import * as http from "node:http";

let requestReceiverMatched = false;
let responseMatched = false;
await new Promise((resolve, reject) => {
  // Intentionally discard the returned server and never close over it. The listener's
  // `this` is the only route to the server object once listen has returned.
  http
    .createServer(function (request, response) {
      requestReceiverMatched =
        typeof this === "object" && this.marker === "active-server";
      responseMatched = request.method === "GET" && request.url === "/receiver";
      response.statusCode = 201;
      response.end("ok");
      if (!requestReceiverMatched) {
        process.exit(1);
        return;
      }
      this.close(resolve);
    })
    .listen(0, function () {
      if (typeof this !== "object") {
        process.exit(1);
        return;
      }
      this.marker = "active-server";
      const { port } = this.address();
      http.get(`http://127.0.0.1:${port}/receiver`, (response) => {
        let body = "";
        response.setEncoding("utf8");
        response.on("data", (chunk) => (body += chunk));
        response.on("end", () => {
          if (response.statusCode !== 201 || body !== "ok") reject(new Error("bad response"));
        });
      }).on("error", reject);
    });
});

if (!requestReceiverMatched || !responseMatched) throw new Error("server listener receiver lost");
console.log("unreferenced HTTP server receiver preserved");
