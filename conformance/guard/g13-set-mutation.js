const s = new Set();
const a = { n: 'a' }, b = { n: 'b' }, c = { n: 'c' };
s.add(a); s.add(b); s.add(c);
const seen = [];
for (const x of s) { seen.push(x.n); s.delete(x) }
console.log(seen.length, seen.join(','), s.size);
