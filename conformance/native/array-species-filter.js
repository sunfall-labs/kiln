// `filter` and `map` create their result through ArraySpeciesCreate, so a subclass gets an
// instance of itself back, and a `Symbol.species` decides what is built. The parallel members are
// those methods (ADR 0068 rule 3), so they answer the same; node has no parallel family, so those
// lines use the sequential method there.
const P = typeof [].parallelMap === "function";
class Tracked extends Array {}
class Plain extends Array {
  static get [Symbol.species]() { return Array; }
}
let built = 0;
class Counting extends Array {
  constructor(...args) { super(...args); built++; }
}
const pick = (name, seq) => (P ? name : seq);
for (const C of [Tracked, Plain, Counting]) {
  const xs = C.from([1, 2, 3, 4, 5, 6]);
  built = 0;
  for (const [name, seq, cb] of [
    ["parallelFilter", "filter", (x) => x % 2 === 0],
    ["parallelMap", "map", (x) => x * 10],
    ["parallelFlatMap", "flatMap", (x) => [x]],
  ]) {
    const seqOut = xs[seq](cb);
    const parOut = xs[pick(name, seq)](cb);
    console.log(C.name, seq, seqOut.constructor.name, seqOut instanceof C, Array.isArray(seqOut),
      String(seqOut), seqOut.length, parOut.constructor.name, String(parOut));
  }
  console.log(C.name, "constructed", built);
}
// A species that builds something odd still receives every kept element, in order, by index.
class Odd extends Array {
  static get [Symbol.species]() {
    return function (n) { const o = { made: n }; return o; };
  }
}
const odd = Odd.from([5, 6, 7, 8]);
console.log("odd filter", JSON.stringify(odd.filter((x) => x > 5)));
console.log("odd map", JSON.stringify(odd.map((x) => x + 1)));
console.log("odd family", JSON.stringify(odd[pick("parallelFilter", "filter")]((x) => x > 5)));
