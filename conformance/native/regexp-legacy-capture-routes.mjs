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
/(other)-(\d+)/.test("other-99");
const retainedValues = [...[firstMatch].values()];
if (retainedValues[0][0] !== "r-5" || RegExp.$1 !== "other" || RegExp.$2 !== "99") {
  throw new Error("ordinary array iterator republished stale matchAll captures");
}
console.log("ordinary-array-values", retainedValues[0][0], RegExp.$1, RegExp.$2);
console.log("match-all-second", iterator.next().value[0], RegExp.$1, RegExp.$2);
console.log("match-all-done", iterator.next().done, RegExp.$1, RegExp.$2);

const indexedMatchAll = /(x)/g;
indexedMatchAll.lastIndex = 1;
const indexedIterator = "ax x".matchAll(indexedMatchAll);
const indexedFirst = indexedIterator.next().value;
if (indexedFirst.index !== 1 || indexedMatchAll.lastIndex !== 1) {
  throw new Error("String.matchAll must begin at and preserve the original lastIndex");
}
console.log("match-all-last-index", indexedFirst.index, indexedMatchAll.lastIndex);

const manyMatches = [..."x".repeat(128).matchAll(/(x)/g)];
if (manyMatches.length !== 128 || manyMatches[127].index !== 127 || manyMatches[127][1] !== "x") {
  throw new Error("matchAll must retain its snapshot and capture ranges across many matches");
}
console.log("match-all-many", manyMatches.length, manyMatches[127].index);

const nonglobal = /x/;
let directRejected = false;
try { "x".matchAll(nonglobal); } catch (error) { directRejected = error instanceof TypeError; }
if (!directRejected) throw new Error("String.matchAll must reject a non-global RegExp");
const symbolIterator = RegExp.prototype[Symbol.matchAll].call(nonglobal, "x");
if (symbolIterator.next().value[0] !== "x") {
  throw new Error("RegExp @@matchAll must accept a non-global RegExp");
}
const nonGlobalWithIndex = /x/;
nonGlobalWithIndex.lastIndex = 2;
const nonGlobalIndexedMatch = RegExp.prototype[Symbol.matchAll]
  .call(nonGlobalWithIndex, "x").next().value;
if (nonGlobalIndexedMatch.index !== 0 || nonGlobalWithIndex.lastIndex !== 2) {
  throw new Error("non-global @@matchAll must ignore the cloned lastIndex during exec");
}
console.log("match-all-symbol-nonglobal", "ok");
const stringPatternMatches = [..."x x".matchAll("x")];
if (stringPatternMatches.length !== 2 || stringPatternMatches[1].index !== 2) {
  throw new Error("String.matchAll must create a global RegExp for non-RegExp patterns");
}

let matchReads = 0;
let flagReads = 0;
const customMatchAll = {
  get [Symbol.match]() { matchReads++; return true; },
  get flags() { flagReads++; return "g"; },
  [Symbol.matchAll](subject) { return { subject }; },
};
const customResult = "subject".matchAll(customMatchAll);
if (customResult.subject !== "subject" || matchReads !== 1 || flagReads !== 1) {
  throw new Error("String.matchAll must preserve custom hooks and observable IsRegExp checks");
}
const unconvertedSubject = { toString() { throw new Error("premature matchAll coercion"); } };
if (String.prototype.matchAll.call(unconvertedSubject, customMatchAll).subject !== unconvertedSubject) {
  throw new Error("String.matchAll must pass its original receiver to the custom hook");
}
const optedOut = /x/;
optedOut[Symbol.match] = false;
if ("x".matchAll(optedOut).next().value[0] !== "x") {
  throw new Error("String.matchAll must honor Symbol.match=false");
}

const splitIndex = /(x)/g;
splitIndex.lastIndex = 1;
"ax".split(splitIndex);
if (splitIndex.lastIndex !== 1) throw new Error("String.split must preserve lastIndex");
console.log("split-last-index", splitIndex.lastIndex);

import { makeModuleMatch, readModuleCaptures } from "./fixtures/regexp-legacy-capture-module.mjs";
console.log("module-write", makeModuleMatch(), RegExp.$1, RegExp.$2);
console.log("module-read", readModuleCaptures());
/(root)-(73)/.test("root-73");
console.log("root-write", readModuleCaptures());
