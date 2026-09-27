// kiln-native: profile=native-release parallelism=1,2,max
//
// The parallel/sequential differential harness (.scratch/parallel-builtins issue 02) in a module
// past the release specialization limit (issue 13): more than 16 functions and 512
// instructions, so `kiln-native` emits every body generic-only. The qualifying `parallelMap`
// sites still get their parallel lowering -- the callback's typed entry and parallel clone, and
// the pooled path at the site -- and `kiln build --profile native-release --explain-parallel`
// reports them `parallel`; the reassigned capture and the property read stay sequential with
// their own reasons. Every answer is `map`'s.
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = i in p === i in m && Object.is(p[i], m[i]);
  const at = i => (Object.is(p[i], -0) ? "-0" : String(p[i]));
  console.log(label, p.length, at(0), at(p.length >> 1), at(p.length - 1), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallelMap differs from map: " + label);
}

// Bulk: ordinary code that makes the module large -- string work, objects, classes, closures.
class Account {
  constructor(name, balance) { this.name = name; this.balance = balance; this.history = []; }
  deposit(n) { this.balance += n; this.history.push(["deposit", n]); return this; }
  withdraw(n) {
    if (n > this.balance) throw new RangeError("insufficient funds in " + this.name);
    this.balance -= n; this.history.push(["withdraw", n]); return this;
  }
  summary() { return this.name + ":" + this.balance.toFixed(2) + ":" + this.history.length; }
}
class Ledger {
  constructor() { this.accounts = new Map(); }
  open(name, balance) { const a = new Account(name, balance); this.accounts.set(name, a); return a; }
  total() { let t = 0; for (const a of this.accounts.values()) t += a.balance; return t; }
  names() { return [...this.accounts.keys()].sort().join(","); }
}
function words(text) { return text.split(/\s+/).filter(w => w.length > 0); }
function capitalize(w) { return w.length ? w[0].toUpperCase() + w.slice(1).toLowerCase() : w; }
function title(text) { return words(text).map(capitalize).join(" "); }
function histogram(text) {
  const counts = {};
  for (const ch of text) counts[ch] = (counts[ch] || 0) + 1;
  return Object.keys(counts).sort().map(k => k + counts[k]).join("");
}
function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
function gcd(a, b) { while (b) { const t = b; b = a % b; a = t; } return a; }
function primes(limit) {
  const out = [];
  for (let n = 2; out.length < limit; n++) {
    let prime = true;
    for (let d = 2; d * d <= n; d++) if (n % d === 0) { prime = false; break; }
    if (prime) out.push(n);
  }
  return out;
}
function matrix(n) {
  const rows = [];
  for (let i = 0; i < n; i++) { const row = []; for (let j = 0; j < n; j++) row.push((i * 7 + j * 3) % 11); rows.push(row); }
  return rows;
}
function multiply(a, b) {
  const n = a.length, out = [];
  for (let i = 0; i < n; i++) {
    const row = [];
    for (let j = 0; j < n; j++) { let s = 0; for (let k = 0; k < n; k++) s += a[i][k] * b[k][j]; row.push(s); }
    out.push(row);
  }
  return out;
}
function trace(m) { let t = 0; for (let i = 0; i < m.length; i++) t += m[i][i]; return t; }
function counter() { let n = 0; return { inc: () => ++n, get: () => n }; }
function memo(f) { const cache = new Map(); return x => { if (!cache.has(x)) cache.set(x, f(x)); return cache.get(x); }; }
function describe(value) {
  switch (typeof value) {
    case "number": return Number.isInteger(value) ? "int" : "float";
    case "string": return "string(" + value.length + ")";
    case "object": return value === null ? "null" : Array.isArray(value) ? "array" : "object";
    default: return typeof value;
  }
}
function tryParse(json) { try { return JSON.parse(json); } catch (e) { return e.name; } }

const ledger = new Ledger();
ledger.open("alice", 100).deposit(50).withdraw(30);
ledger.open("bob", 20).deposit(5);
let refused = "";
try { ledger.accounts.get("bob").withdraw(1000); } catch (e) { refused = e.message; }
console.log(ledger.names(), ledger.total(), ledger.accounts.get("alice").summary(), refused);
console.log(title("the quick  brown fox"), histogram("mississippi"), fib(15), gcd(1071, 462));
console.log(primes(12).join(" "), trace(multiply(matrix(5), matrix(5))));
const c = counter(); c.inc(); c.inc();
const sq = memo(x => x * x);
console.log(c.get(), sq(12), sq(12), [1, "ab", null, [], {}, 1.5, undefined].map(describe).join(","));
console.log(tryParse('{"a":[1,2]}').a.length, tryParse("{bad"));

// The sites.
const xs = [];
for (let i = 0; i < 17000; i++) xs.push(i - 8500);
check("arith", xs.parallelMap(x => x * 3 - 1), xs.map(x => x * 3 - 1));
const rate = 1.2;
check("capture", xs.parallelMap(x => x * rate), xs.map(x => x * rate));
check("index-math", xs.parallelMap((x, i) => Math.sqrt(Math.abs(x)) + i), xs.map((x, i) => Math.sqrt(Math.abs(x)) + i));
function scaled(k) { return xs.parallelMap(x => x / k); }
check("in-function", scaled(-4), xs.map(x => x / -4));
const f64 = new Float64Array(xs);
check("typed-array", Array.from(f64.parallelMap(x => x * 0.5)), Array.from(f64.map(x => x * 0.5)));
let bump = 1;
bump = 2;
check("reassigned", xs.parallelMap(x => x + bump), xs.map(x => x + bump));
const box = { k: 3 };
check("property", xs.parallelMap(x => x * box.k), xs.map(x => x * box.k));
