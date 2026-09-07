// A parsed object key must survive its own walk, however deep the document.
//
// `kiln_c99_json_build` keeps 32 open containers on the machine stack, where the collector's
// conservative scan roots whatever they hold. Past that the frames move to `malloc`, which the
// collector never looks at, and every pending key is mirrored into an array of this heap
// instead. The mirror used to store `kiln_val_from_boundary(key)`, and for a string that
// *allocates a second block and copies the bytes* -- so it rooted a copy while the frame went on
// pointing into the original, which was then swept and handed to the next key. The key came back
// as another key's bytes.
//
// WHY THIS PROBE NEEDS `mise run native-stress` TO MEAN ANYTHING. It needs a collection inside
// the window between building a key and defining it. Ordinarily that never happens and every
// line below passes on a compiler that has the bug -- which is exactly what
// `json-shaped-build-edges.js` did while nesting 200 deep. Under `KILN_GC_STRESS` the collection
// lands on every allocation and the same lines fail immediately.
const q = String.fromCharCode(34);

function nest(depth, distinct) {
  let s = "1";
  for (let i = depth - 1; i >= 0; i--) {
    s = "{" + q + (distinct ? "k" + i : "d") + q + ":" + s + "}";
  }
  return s;
}

// Either side of the spill (32), and well past it. Distinct keys are the case that fails: with one
// repeated key the intern table holds the bytes and a stale read lands on the same spelling.
for (const depth of [8, 31, 32, 33, 40, 64, 200]) {
  for (const distinct of [false, true]) {
    const src = nest(depth, distinct);
    const round = JSON.stringify(JSON.parse(src));
    console.log(depth, distinct ? "distinct" : "same", round === src ? "roundtrip" : "DIFFERS " + round);
  }
}

// The keys themselves, read back past the spill: an empty key, one that needs escaping, one that
// is a prefix of the next, and a non-ASCII one -- each of which the walk stores as a block and
// interns at delivery rather than copying twice.
const deep = JSON.parse(nest(50, true));
let node = deep;
const seen = [];
while (typeof node === "object" && node !== null) {
  const key = Object.keys(node)[0];
  seen.push(key);
  node = node[key];
}
console.log(seen.length, seen[0], seen[31], seen[32], seen[49], node);

// The same, with the innermost object carrying keys the walk has to get exactly right: an empty
// one, one needing an escape, one that is a prefix of another, and a non-ASCII one.
const letters = "abcdefghijklmnopqrstuvwxyzABCDEF";
let innerText = '{"":1,"G H":2,"G":3,"\\u00e9":4}';
for (let i = letters.length - 1; i >= 0; i--) {
  innerText = "{" + q + letters[i] + q + ":" + innerText + "}";
}
let inner = JSON.parse(innerText);
for (const k of letters) {
  inner = inner[k];
}
console.log(JSON.stringify(Object.keys(inner)), inner[""], inner["G H"], inner.G, inner["\u00e9"]);
