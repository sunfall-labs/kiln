import { runInNewContext } from "node:vm";

const result = runInNewContext("let n = x + 1; n * 2", { x: 4 });
if (result !== 10) {
  throw new Error(`new-context completion was ${result}, expected 10`);
}
console.log("new-context completion passed");
