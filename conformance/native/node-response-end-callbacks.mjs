import http from "node:http";

const callbacks = [];
const server = http.createServer((request, response) => {
  response.once("finish", () => callbacks.push(`finish:${request.url}`));
  if (request.url === "/callback-only") {
    response.writeHead(204, { "x-response": "callback-only" });
    response.end(() => callbacks.push("only"));
    return;
  }
  response.writeHead(200, { "content-type": "text/plain" });
  response.end("three-argument", "utf8", () => callbacks.push("three"));
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const port = server.address().port;

const get = (path) => new Promise((resolve, reject) => {
  const request = http.get({ host: "127.0.0.1", port, path }, (response) => {
    let body = "";
    response.setEncoding("utf8");
    response.on("data", (chunk) => body += chunk);
    response.on("end", () => resolve([response.statusCode, body]));
  });
  request.on("error", reject);
});
console.log("only", ...(await get("/callback-only")));
console.log("three", ...(await get("/three")));
console.log("callbacks", callbacks.join(","));
await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
console.log("closed");
