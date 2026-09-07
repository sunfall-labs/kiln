// A small array starts life in its own inline element storage; every way of growing it past
// that storage must move the elements out, never hand the inline pointer to the allocator.
const out = [];
function show(a) { return a.length + ":" + a.join(","); }
const parts = "a b c".split(" ");
for (let i = 0; i < 12; i++) parts.push("p" + i);
out.push(show(parts));
const sliced = [1, 2, 3, 4, 5, 6, 7, 8, 9].slice(2);
sliced.push(10, 11, 12, 13);
sliced.unshift(0);
out.push(show(sliced));
const mapped = [1, 2, 3].map((x) => x * 2);
mapped.length = 12;
mapped[11] = "end";
out.push(show(mapped));
const filtered = [1, 2, 3, 4].filter((x) => x % 2 === 0);
filtered.splice(1, 0, "s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8");
out.push(show(filtered));
const from = Array.from("xyz");
from[20] = "far";
out.push(show(from).replace(/,{2,}/g, (m) => "<" + (m.length) + ">"));
const conc = "q,w".split(",").concat(["e", "r", "t", "y", "u", "i", "o", "p"]);
conc.push("last");
out.push(show(conc));
const spread = [..."1234".split("")];
for (let i = 0; i < 10; i++) spread.push(i);
out.push(show(spread));
const emptyish = "".split(",");
for (let i = 0; i < 9; i++) emptyish[i + 1] = i;
out.push(show(emptyish));
const limited = "a-b-c-d-e-f-g-h-i-j".split("-", 3);
limited.push(...[1, 2, 3, 4, 5, 6, 7]);
out.push(show(limited));
const rev = "1 2 3".split(" ");
rev.push(4, 5, 6, 7, 8, 9);
rev.reverse();
out.push(show(rev) + " " + JSON.stringify(rev.slice(-3)));
let total = 0;
for (let round = 0; round < 200; round++) {
  const p = "0 1 2 3 4 5 6 7".split(" ");
  p.push("8");
  p.pop();
  total += p.length;
}
out.push("total:" + total);
console.log(out.join("\n"));
