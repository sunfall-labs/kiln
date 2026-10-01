const http = require("node:http");
const requestListener = function requestListener() {};

const withOptions = http.createServer({}, requestListener);
const registered = withOptions.listeners("request");
if (registered.length !== 1 || registered[0] !== requestListener) {
  throw new Error("createServer(options, listener) must preserve and register the listener");
}

const withoutListener = http.createServer();
if (withoutListener.listeners("request").length !== 0) {
  throw new Error("createServer() must allow a request listener to be added later");
}
const lateListener = function lateListener() {};
withoutListener.on("request", lateListener);
if (withoutListener.listeners("request")[0] !== lateListener) {
  throw new Error("createServer() must preserve later request listener identity");
}
console.log("http createServer overloads ok");
