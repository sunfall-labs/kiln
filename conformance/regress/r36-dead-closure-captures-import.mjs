// A function nothing calls still captures the imports it names (realms issue 22). The linker
// drops an import that nothing live reads, and the interpreter then failed the module before its
// first line with "local slot 0 does not hold a captured cell": the hoisted declaration below
// captures `readFileSync`'s slot when the body starts, whether or not it is ever called.
import { readFileSync } from "node:fs";
import * as path from "node:path";
import { format } from "node:util";

function neverCalled() {
  return readFileSync("/nonexistent");
}
const neverCalledArrow = () => path.join("a", "b");

// A live capture of an import next to the dead ones still sees the exporter's binding.
const live = () => format("%s-%d", "live", 7);
console.log(live());
console.log("done");
