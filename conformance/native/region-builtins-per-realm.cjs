// Realms issue 08: the runtime bootstrap's top-level closures -- `Object.groupBy`, `Map.groupBy`,
// `Intl.Segmenter` among them -- are made once per process in the shared immutable region and
// handed to every realm, instead of one copy per realm. What a program can change about one is
// still its realm's own: a property written onto the function, its `prototype` and what is put
// there, and whether it is extensible. A worker is another realm of the same process, so it must see none of
// what the main realm did, and the main realm must still see all of it after the worker ran.
const { Worker, isMainThread, parentPort } = require("node:worker_threads");

function report(who) {
  const groupBy = Object.groupBy;
  const Segmenter = Intl.Segmenter;
  const made = new Segmenter("en", { granularity: "word" });
  return [
    who,
    typeof groupBy,
    groupBy.name,
    groupBy.length,
    groupBy === Object.groupBy,
    String(groupBy.marker),
    String(Segmenter.marker),
    String(Segmenter.prototype.extra),
    typeof made.segment,
    made.resolvedOptions().granularity,
    Object.getPrototypeOf(made) === Segmenter.prototype,
    Object.isExtensible(Map.groupBy),
    JSON.stringify(Object.groupBy([1, 2, 3], (n) => (n % 2 ? "odd" : "even"))),
  ].join(" ");
}

if (isMainThread) {
  console.log(report("main before"));
  Object.groupBy.marker = "main";
  Intl.Segmenter.marker = "segmenter";
  Intl.Segmenter.prototype.extra = 1;
  Object.preventExtensions(Map.groupBy);
  console.log(report("main after"));
  const worker = new Worker(__filename);
  worker.on("message", (message) => console.log(message));
  worker.on("exit", (code) => console.log("worker exit", code, "|", report("main end")));
} else {
  parentPort.postMessage(report("worker"));
}
