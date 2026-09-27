// native-env: KILN_EXTENSIONS=0
//
// ADR 0068 rule 4: with `KILN_EXTENSIONS=0` both engines install exactly Node's surface -- no
// kiln extension on `Array.prototype` or `%TypedArray%.prototype`, a read of one is `undefined`,
// a call of one is Node's TypeError, and the bridge that told the bootstrap is gone. The first
// line is read by `scripts/native.sh`, which runs both engines with that environment.
//
// `NODE_ARRAY_PROTOTYPE` is Node v26's `Object.getOwnPropertyNames(Array.prototype)`, sorted.
// The typed-array prototype is compared only on the extension: its own list differs from Node's
// for reasons that have nothing to do with this switch.
const NODE_ARRAY_PROTOTYPE = [
  "at", "concat", "constructor", "copyWithin", "entries", "every", "fill", "filter", "find",
  "findIndex", "findLast", "findLastIndex", "flat", "flatMap", "forEach", "includes", "indexOf",
  "join", "keys", "lastIndexOf", "length", "map", "pop", "push", "reduce", "reduceRight",
  "reverse", "shift", "slice", "some", "sort", "splice", "toLocaleString", "toReversed",
  "toSorted", "toSpliced", "toString", "unshift", "values", "with",
];
const names = Object.getOwnPropertyNames(Array.prototype).sort();
console.log("parallelMap" in [], names.includes("parallelMap"), typeof ([] as any).parallelMap);
console.log(JSON.stringify(names) === JSON.stringify(NODE_ARRAY_PROTOTYPE), names.length);
console.log(Object.getOwnPropertyDescriptor(Array.prototype, "parallelMap") === undefined);
console.log(Object.prototype.hasOwnProperty.call(Array.prototype, "parallelMap"));
console.log(typeof (Array.prototype as any).parallelMap, Reflect.ownKeys(Array.prototype).length);
console.log("parallelMap" in Array.prototype, Object.hasOwn(Array.prototype, "parallelMap"));

const typedProto = Object.getPrototypeOf(Uint8Array.prototype);
console.log("parallelMap" in typedProto, Object.getOwnPropertyNames(typedProto).includes("parallelMap"));
console.log(typeof (new Float64Array(2) as any).parallelMap);

// A call site that would take either lowering of `parallelMap` -- a pure numeric callback over
// a numeric array, and over a typed array -- must not run it.
const big = Array.from({ length: 4096 }, (_, i) => i);
try {
  (big as any).parallelMap((x: number) => x * 2);
  console.log("array called");
} catch (e) {
  console.log("array", e instanceof TypeError);
}
try {
  (new Float64Array(big) as any).parallelMap((x: number) => x * 2);
  console.log("typed called");
} catch (e) {
  console.log("typed", e instanceof TypeError);
}
console.log("__kilnExtensions" in globalThis);
