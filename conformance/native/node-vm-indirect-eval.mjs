import { runInNewContext } from "node:vm";

console.log(runInNewContext("1 + 2"));
console.log(runInNewContext("globalThis.Math.max(7, 9)"));
