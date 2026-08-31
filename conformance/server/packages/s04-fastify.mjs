import Fastify from "fastify";
import http from "node:http";

const app = Fastify();

app.get("/", async () => ({ ok: true }));
app.get("/users/:id", async (request) => ({ id: request.params.id }));
app.post("/echo", async (request) => ({ got: request.body }));
app.setNotFoundHandler(async (request, reply) => reply.code(404).send({ error: "not found" }));

function send(port, method, path, payload) {
  return new Promise((resolve) => {
    const headers = payload ? { "content-type": "application/json" } : {};
    const request = http.request({ host: "127.0.0.1", port, method, path, headers }, (res) => {
      let body = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => { body += chunk; });
      res.on("end", () => resolve([res.statusCode, body]));
    });
    if (payload) request.write(JSON.stringify(payload));
    request.end();
  });
}

await app.listen({ port: 0, host: "127.0.0.1" });
const { port } = app.server.address();

for (const [method, path, payload] of [
  ["GET", "/", null],
  ["GET", "/users/7", null],
  ["POST", "/echo", { hello: "kiln" }],
  ["GET", "/missing", null],
]) {
  const [status, body] = await send(port, method, path, payload);
  console.log(status, body);
}

await app.close();
