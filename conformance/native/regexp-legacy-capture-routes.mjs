// String regexp methods publish the most recent successful capture. Failed searches preserve it.
const search = /([a-z]+)-(\d+)/;
console.log("search", "one-17".search(search), RegExp.$1, RegExp.$2);
console.log("search-miss", "nothing".search(search), RegExp.$1, RegExp.$2);

const split = "a-3,b-29".split(/([a-z]+)-(\d+)/, 1);
console.log("split-limit", split.join("|"), RegExp.$1, RegExp.$2);

const replaced = "x-41".replace(/([a-z]+)-(\d+)/, "$2:$1");
console.log("replace", replaced, RegExp.$1, RegExp.$2);

const iterator = "r-5,s-62".matchAll(/([a-z]+)-(\d+)/g);
console.log("match-all-before-next", RegExp.$1, RegExp.$2);
const firstMatch = iterator.next().value;
console.log("match-all-first", firstMatch[0], RegExp.$1, RegExp.$2);
/other-(\d+)/.test("other-99");
const retainedValues = [...[firstMatch].values()];
if (retainedValues[0][0] !== "r-5" || RegExp.$1 !== "other" || RegExp.$2 !== "99") {
  throw new Error("ordinary array iterator republished stale matchAll captures");
}
console.log("ordinary-array-values", retainedValues[0][0], RegExp.$1, RegExp.$2);
console.log("match-all-second", iterator.next().value[0], RegExp.$1, RegExp.$2);
console.log("match-all-done", iterator.next().done, RegExp.$1, RegExp.$2);

import { makeModuleMatch, readModuleCaptures } from "./fixtures/regexp-legacy-capture-module.mjs";
console.log("module-write", makeModuleMatch(), RegExp.$1, RegExp.$2);
console.log("module-read", readModuleCaptures());
/(root)-(73)/.test("root-73");
console.log("root-write", readModuleCaptures());
