// A loop bounded by an array's length steps its counter without an overflow check; the bound
// is still read every iteration, so a loop that grows or shrinks its array mid-way, starts
// negative, steps by more than one, or counts down must behave exactly as node.
const out = [];
const a = [1, 2, 3];
let n = 0;
for (let i = 0; i < a.length; i++) { if (a.length < 8) a.push(i); n += a[i]; }
out.push(n + ":" + a.length);
const b = [5, 6, 7, 8, 9];
let m = 0;
for (let i = 0; i < b.length; i++) { m += b[i]; if (i === 1) b.pop(); }
out.push(m + ":" + b.length);
const c = [1, 2, 3, 4, 5, 6];
let s2 = 0;
for (let i = -2; i < c.length; i += 2) s2 += (c[i] === undefined ? 100 : c[i]);
out.push(s2);
let d = 0;
for (let i = c.length - 1; i >= 0; i--) d = d * 10 + c[i];
out.push(d);
let e = 0;
for (let i = 0; i <= c.length; i++) e += i;
out.push(e);
const big = new Array(70000).fill(1);
let f = 0;
for (let i = 0; i < big.length; i++) f += big[i];
out.push(f);
let g = 0;
for (let i = 0; i < c.length; i++) { for (let j = i; j < c.length; j++) g += c[j]; }
out.push(g);
const holes = [1, , 3];
let h = 0;
for (let i = 0; i < holes.length; i++) h += holes[i] === undefined ? 1000 : holes[i];
out.push(h);
console.log(out.join(","));
