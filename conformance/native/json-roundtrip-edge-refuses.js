// A replacer and a reviver are both native backend refusals today, unrelated to the JSON
// performance work in this change: see `KILN_C99_REFUSE("JSON.stringify with a replacer")` and
// `KILN_C99_REFUSE("JSON.parse with a reviver")` in prelude_core.c. Kept separate from
// `json-roundtrip-edge.js` so that this expected refusal does not swallow the rest of that file's
// coverage under native.sh's one-verdict-per-file classification.
console.log(JSON.stringify({ a: 1, b: 2, c: 3 }, ["a", "c"]));
console.log(JSON.stringify({ a: 1, b: 2 }, (key, value) => (key === "b" ? undefined : value)));
console.log(JSON.parse('{"a":1}', (key, value) => value));
