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

for (const listener of [undefined, null, false, 0, ""]) {
  if (http.createServer({}, listener).listeners("request").length !== 0) {
    throw new Error("falsy optional request listeners must be ignored");
  }
}
for (const listener of [1, {}, "listener"]) {
  let rejected = false;
  try { http.createServer({}, listener); } catch (error) { rejected = error instanceof TypeError; }
  if (!rejected) throw new Error("truthy invalid request listener must throw TypeError");
}
