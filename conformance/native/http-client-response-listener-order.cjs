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
    assert(response.socket === request.socket, "response exposes the assigned socket");
    response.resume();
    response.on("end", () => {
      assert(order.join(",") === "callback,event", "callback precedes later response listeners");
      let removedCallbackRan = false;
      function removedCallback() { removedCallbackRan = true; }
      const second = http.get(
        { host: "127.0.0.1", port: server.address().port }, removedCallback,
      );
      second.removeListener("response", removedCallback);
      second.on("response", (incoming) => {
        assert(!removedCallbackRan, "removing the optional callback removes its event listener");
        incoming.resume();
        incoming.on("end", () => server.close(() =>
          console.log("HTTP response callback and event share one ordered emission")));
      });
      second.on("error", (error) => { throw error; });
    });
  });
  request.on("error", (error) => { throw error; });
});
