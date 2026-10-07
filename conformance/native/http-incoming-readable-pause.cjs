const http = require("node:http");

function assert(value, message) {
  if (!value) throw new Error(message);
}

const server = http.createServer((_request, response) => response.end("paused-body"));
server.listen(0, "127.0.0.1", () => {
  const request = http.get({ host: "127.0.0.1", port: server.address().port });
  request.on("response", (incoming) => {
    incoming.pause();
    const chunks = [];
    let ends = 0;
    incoming.on("readable", () => {
      let chunk;
      while ((chunk = incoming.read()) !== null) chunks.push(chunk.toString());
    });
    incoming.on("end", () => {
      ends++;
      assert(chunks.join("") === "paused-body", "paused response data is buffered and readable");
      assert(ends === 1, "Readable emits end exactly once after buffered data drains");
      server.close(() => console.log("HTTP IncomingMessage pause/read/end shares Readable state"));
    });
  });
  request.on("error", (error) => { throw error; });
});
