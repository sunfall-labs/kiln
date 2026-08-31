const root = new URL("/", "http://127.0.0.1");
const echo = new URL("/echo?q=hello+kiln&encoded=%2Fok", "http://127.0.0.1:8080");

if (root.pathname !== "/" || root.searchParams.get("missing") !== null) {
  throw new Error("root URL");
}
if (echo.origin !== "http://127.0.0.1:8080") throw new Error("origin");
if (echo.pathname !== "/echo") throw new Error("pathname");
if (echo.searchParams.get("q") !== "hello kiln") throw new Error("form decoding");
if (echo.searchParams.get("encoded") !== "/ok") throw new Error("percent decoding");

console.log("pass");
