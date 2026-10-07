// Legacy subclasses in packages such as light-my-request initialize an existing receiver with
// ServerResponse.call(this, req), then use its inherited writable and header surface.
import http from "node:http";

const response = Object.create(http.ServerResponse.prototype);
http.ServerResponse.call(response, { method: "GET" });
response.setHeader("X-Legacy-Probe", "ok");
if (response.req.method !== "GET" || response.getHeader("x-legacy-probe") !== "ok" ||
    response.constructor !== http.ServerResponse ||
    !(response instanceof http.ServerResponse) || typeof response.on !== "function" ||
    http.ServerResponse.prototype.setHeader.length !== 2) {
  throw new Error("legacy ServerResponse initialization lost state, identity, or method shape");
}
console.log(
  response.req.method,
  response.getHeader("x-legacy-probe"),
  response.constructor === http.ServerResponse,
  response instanceof http.ServerResponse,
  typeof response.on,
  http.ServerResponse.prototype.setHeader.length,
);
