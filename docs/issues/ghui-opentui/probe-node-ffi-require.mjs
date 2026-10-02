import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const ffi = require("node:ffi");
console.log(typeof ffi.dlopen, ffi.suffix);
