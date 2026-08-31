import { parse as parseUrl } from "node:url";
import { parse as parseQuery } from "node:querystring";

const parsed = parseUrl("/hello?hello=world&foo=b==ar");
const query = parseQuery(parsed.query);

if (parsed.pathname !== "/hello") throw new Error("legacy URL pathname mismatch");
if (query.hello !== "world") throw new Error("querystring value mismatch");
if (query.foo !== "b==ar") throw new Error("querystring embedded equals mismatch");
console.log("pass");
