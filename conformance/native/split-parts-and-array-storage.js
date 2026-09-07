// `String.prototype.split` on a one-byte separator, and the element storage the array it makes
// now carries inside its own block.
//
// Both were rewritten together. `split` on an ASCII string cut by one ASCII byte is the shape
// the emitted C has an intrinsic for and the shape the dispatch reaches most often, and it now
// runs as one pass that records the parts as it finds them, sizes the array from that count, and
// takes all the parts' blocks out of one nursery reservation. An array made with a known count
// larger than its own eight inline slots keeps its elements in the room past the end of its own
// block rather than in a `malloc`, so growing past that count must copy rather than `realloc`,
// and the collector must not hand the run to `free`.
//
// So the cases here are the ones where those two decisions differ from the ones they replaced:
// the sizes either side of the eight-slot inline run and either side of the sixty-four-part scan
// buffer, every way a part can be empty, every way a limit can truncate, and then the array put
// through each of the growth paths that used to test `elements == inline_elements` on its own.
// The parts are also read back the ways that reach for raw bytes, because a part is a fresh
// string block and not a view into its source.

const line = "2026-09-03T20:57:34.123Z [INFO] service=payment k=v amount=149.99 rest=1 more=2 last=3";

// The shape the intrinsic is for.
const parts = line.split(" ");
console.log(parts.length, parts[0], parts[parts.length - 1]);
console.log(parts.join("|"));

// Every way a part can be empty, and a separator that is not there at all.
console.log(JSON.stringify("".split(" ")));
console.log(JSON.stringify("a".split(" ")));
console.log(JSON.stringify(" ".split(" ")));
console.log(JSON.stringify("  ".split(" ")));
console.log(JSON.stringify(" a".split(" ")));
console.log(JSON.stringify("a ".split(" ")));
console.log(JSON.stringify("a  b".split(" ")));
console.log(JSON.stringify("abc".split(" ")));
console.log(JSON.stringify("a,b,c".split(",")));

// A limit truncates rather than merging the tail into the last kept piece.
console.log(JSON.stringify("a b c d".split(" ", 0)));
console.log(JSON.stringify("a b c d".split(" ", 1)));
console.log(JSON.stringify("a b c d".split(" ", 3)));
console.log(JSON.stringify("a b c d".split(" ", 4)));
console.log(JSON.stringify("a b c d".split(" ", 99)));
console.log(JSON.stringify("a b c d".split(" ", -1).length));
console.log(JSON.stringify("a b c d".split(" ", NaN)));

// The eight-slot inline run and the sixty-four-part scan buffer, on both sides of each.
function counted(n) {
  const cells = [];
  for (let i = 0; i < n; i += 1) cells.push("p" + i);
  return cells.join(" ");
}
for (const n of [1, 7, 8, 9, 16, 63, 64, 65, 200]) {
  const got = counted(n).split(" ");
  console.log(n, got.length, got[0], got[n - 1], got.join("").length);
}

// A separator of more than one byte, and one that is a regular expression: neither takes the
// one-byte path, and both must still answer the same way.
console.log(JSON.stringify("a::b::c".split("::")));
console.log(JSON.stringify("a1b2c".split(/[0-9]/)));
console.log(JSON.stringify("a b c".split("")));
console.log(JSON.stringify("abc".split(undefined)));

// Text that is not ASCII: `length` counts code units, not bytes, and a part carries its own.
const wide = "héllo wörld ünïcode";
const wideParts = wide.split(" ");
console.log(wideParts.length, wideParts[0], wideParts[0].length, wideParts[2].length);
console.log(JSON.stringify(wideParts));
console.log("naïve x".split(" ")[0].length, "naïve".length);

// A part is a string like any other: equal to a flat one, usable as a key, and hashable.
const key = "amount=149.99".split("=")[0];
console.log(key === "amount", key.length, typeof key);
const holder = {};
holder[key] = 1;
holder["amount"] = (holder["amount"] || 0) + 1;
console.log(JSON.stringify(holder), Object.keys(holder).length);
const seen = new Set(["amount"]);
console.log(seen.has(key), new Map([[key, 7]]).get("amount"));

// The ways that reach for the raw bytes behind a part.
const encoded = new TextEncoder().encode(parts[1]);
console.log(parts[1], encoded.length, Array.from(encoded).join(","));
console.log(JSON.stringify(parts.slice(0, 3)));
console.log(/^\[INFO\]$/.test(parts[1]), parts[2].replace("=", ":"));
console.log(parts[0].slice(0, 4), parts[0].substring(5, 7), parts[0].indexOf("T"));

// The array `split` hands back, through every growth path that used to ask whether the elements
// were the array's own inline eight.
const grown = line.split(" ");
console.log(grown.length);
grown.push("pushed");
console.log(grown.length, grown[grown.length - 1]);
grown[grown.length + 2] = "sparse";
console.log(grown.length, grown[grown.length - 1], grown[grown.length - 2]);
grown.length = 4;
console.log(grown.length, grown.join("|"));
grown.length = 12;
console.log(grown.length, JSON.stringify(grown[11]));
console.log(grown.slice(0, 3).join("|"), grown.concat(["x"]).length);

const sortable = "delta alpha charlie bravo echo golf foxtrot india hotel".split(" ");
console.log(sortable.length, sortable.slice().sort().join(","));
console.log(sortable.splice(2, 3).join(","), sortable.join(","));
console.log(sortable.reverse().join(","), sortable.pop(), sortable.length);

// An array made at each size around the inline run, then grown out of it.
for (const n of [7, 8, 9, 64, 65]) {
  const made = counted(n).split(" ");
  made.push("tail");
  made.push("tail2");
  console.log(n, made.length, made[n], made[n + 1], made[0], made[n - 1]);
}

// The parts outliving the string they came from, with the collector run in between.
function partsOf() {
  return ("x=" + Math.floor(1) + " y=2 z=3 w=4 v=5 u=6 t=7 s=8 r=9 q=10").split(" ");
}
const kept = partsOf();
let churn = 0;
for (let i = 0; i < 2000; i += 1) churn += ("a" + i).split("a").length;
console.log(churn, kept.length, kept.join("|"));
