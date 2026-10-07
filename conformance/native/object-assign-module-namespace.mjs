import * as source from "./fixtures/module-namespace-object-assign.mjs";

source.replaceParsers();
const copied = Object.assign({}, source);
console.log(copied.parsers === source.parsers);
