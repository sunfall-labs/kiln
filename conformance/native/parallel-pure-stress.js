// kiln-native: profile=native-release
//
// Tier 4 (docs/issues/parallel-builtins issue 09) for `mise run native-stress`: small, no
// thread-count directive, and checking its answers without allocating. The pure built-ins build
// strings and arrays in the workers' arenas -- `trim`, `split`, `map(Number)`, `slice`,
// `padStart`, `toFixed`, `JSON.stringify`, and a helper's `map` over a closure -- so under
// collector stress every one of them must be reachable through the result from the first
// allocation after the join, and nothing may collect while the arenas are open.
const n = typeof process !== "undefined" && process.env.KILN_GC_STRESS ? 1200 : 20000;
const lines = [];
for (let i = 0; i < n; i++) lines.push(" " + i + "," + (i % 7) + ",x" + (i % 3) + " ");

let ok = true;
const parsed = lines.parallelMap(l => l.trim().split(",").map(Number));
for (let i = 0; i < n; i++) {
  const row = parsed[i];
  ok = ok && row.length === 3 && row[0] === i && row[1] === i % 7 && row[2] !== row[2];
}
console.log("parsed", ok);

const fields = lines.parallelMap(l => l.trim().split(",").slice(1));
ok = true;
for (let i = 0; i < n; i++) ok = ok && fields[i].length === 2 && fields[i][1] === "x" + (i % 3);
console.log("fields", ok);

function cell(s, w) { return s.trim().padStart(w, "0"); }
const padded = lines.parallelMap(l => l.split(",").map(s => cell(s, 6)));
ok = true;
for (let i = 0; i < n; i++) ok = ok && padded[i][2] === "0000x" + (i % 3) && padded[i][0].length === 6;
console.log("padded", ok);

const json = lines.parallelMap((l, i) => JSON.stringify({ i, f: (i / 8).toFixed(3), l: l.trim() }));
ok = true;
for (let i = 0; i < n; i++) ok = ok && json[i].length > 20 && json[i].charCodeAt(1) === 34 && json[i].endsWith(" ") === false;
console.log("json", ok);
