const http = require("node:http");

function assert(value, message) {
  if (!value) throw new Error(message);
}

const server = http.createServer((_request, response) => response.end("ok"));
server.listen(0, "127.0.0.1", () => {
  let request;
  let callbackResponse;
  const order = [];
  request = http.get(
    { host: "127.0.0.1", port: server.address().port },
    function (response) {
      assert(this === request, "response callback receiver is the ClientRequest");
      order.push("callback");
      callbackResponse = response;
    },
  );
  request.on("response", (response) => {
    assert(response === callbackResponse, "callback and event share IncomingMessage");
    order.push("event");
    response.resume();
    response.on("end", () => {
      assert(order.join(",") === "callback,event", "callback precedes later response listeners");
      assert(response.socket === request.socket, "response exposes the assigned socket");
      server.close(() => console.log("HTTP response callback and event share one ordered emission"));
    });
  });
  request.on("error", (error) => { throw error; });
});
