// Sync generators have @@iterator but no @@asyncIterator. Libraries such as p-map use this
// distinction to choose their synchronous-iterable fallback.
const iterator = (function* () { yield 1; })();
if (iterator[Symbol.iterator]() !== iterator) throw new Error("sync iterator method missing");
if (iterator[Symbol.asyncIterator] !== undefined) throw new Error("sync generator has async iterator");
console.log("sync generator has no async iterator");
