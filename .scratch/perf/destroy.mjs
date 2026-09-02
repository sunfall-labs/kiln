import { createServer } from "node:http";
const server = createServer((request, response) => {
  console.log("destroy");
  request.socket.destroy();
  server.close(() => console.log("closed"));
});
server.listen(18919, "127.0.0.1", () => console.log("listening"));
