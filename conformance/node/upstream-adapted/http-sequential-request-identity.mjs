import http from "node:http";

const server = http.createServer((request, response) => {
  response.end(request.url);
});

server.listen(0, "127.0.0.1", () => {
  const paths = [];
  const request = path => http.get({
    host: "127.0.0.1",
    port: server.address().port,
    path,
  }, response => {
    let body = "";
    response.setEncoding("utf8");
    response.on("data", chunk => body += chunk);
    response.on("end", () => {
      paths.push(body);
      if (paths.length === 1) request("/second");
      else {
        if (paths.join(",") !== "/first,/second") throw new Error("response identity");
        console.log("pass");
        server.close();
      }
    });
  });
  request("/first");
});
