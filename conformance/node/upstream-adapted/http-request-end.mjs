import http from "node:http";

const expected = "Post Body For Test";
const server = http.createServer((req, res) => {
  let result = "";
  req.setEncoding("utf8");
  req.on("data", (chunk) => { result += chunk; });
  req.on("end", () => {
    if (result !== expected) throw new Error(`wrong request body: ${result}`);
    res.writeHead(200);
    res.end("hello world\n");
    server.close();
  });
});

server.listen(0, "127.0.0.1", () => {
  const request = http.request({
    host: "127.0.0.1",
    port: server.address().port,
    path: "/",
    method: "POST",
  }, (response) => {
    if (response.statusCode !== 200) throw new Error(`wrong status: ${response.statusCode}`);
    response.resume();
  });
  if (request.end(expected) !== request) throw new Error("end must return the ClientRequest");
});

console.log("pass");
