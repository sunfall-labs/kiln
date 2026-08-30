import { load, dump } from "js-yaml";
const doc = load("name: kiln\nitems:\n  - 1\n  - two\nnested:\n  flag: true\n");
console.log(JSON.stringify(doc));
console.log(dump({ a: [1, 2], b: "x" }).trim().split("\n").join("|"));
