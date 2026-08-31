import express from "express";
import http from "node:http";

function send(port, method, path, payload) {
  return new Promise((resolve, reject) => {
    const body = payload === undefined ? undefined : JSON.stringify(payload);
    const headers = body === undefined
      ? {}
      : { "content-type": "application/json", "content-length": String(Buffer.byteLength(body)) };
    const request = http.request({ host: "127.0.0.1", port, method, path, headers }, (response) => {
      let received = "";
      response.setEncoding("utf8");
      response.on("data", (chunk) => { received += chunk; });
      response.on("end", () => resolve([response.statusCode, received, response.headers["x-kiln"]]));
    });
    request.on("error", reject);
    if (body !== undefined) request.write(body);
    request.end();
  });
}

async function main() {
  const app = express();
  app.use((request, response, next) => {
    response.set("x-kiln", "express");
    next();
  });
  app.use(express.json());
  app.get("/", (request, response) => response.json({ ok: true }));
  app.get("/users/:id", (request, response) => {
    response.json({ id: request.params.id, q: request.query.q });
  });
  app.post("/echo", (request, response) => response.status(201).json({ got: request.body }));

  const server = app.listen(0, "127.0.0.1");
  await new Promise((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  const { port } = server.address();

  for (const [method, path, payload] of [
    ["GET", "/", undefined],
    ["GET", "/users/7?q=kiln", undefined],
    ["POST", "/echo", { hello: "kiln" }],
    ["GET", "/missing", undefined],
  ]) {
    const [status, body, marker] = await send(port, method, path, payload);
    console.log(status, marker, body);
  }

  await new Promise((resolve, reject) => {
    server.close((error) => error ? reject(error) : resolve());
  });
}

main();
