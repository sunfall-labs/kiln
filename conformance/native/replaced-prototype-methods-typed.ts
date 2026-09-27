// kiln-native: profile=native-release
//
// Parallel-builtins issue 30, item 1, in a release build: bodies compiled typed -- a proven string
// receiver, a proven array, an integer result, a typed array's `map` -- see a replaced built-in
// method, including one whose replacement answers a different type than the built-in does, and
// the built-in again once it is put back (or, for the typed array, deleted from the prototype).
function cat(a: unknown[], sep: string): string {
  let out = "";
  for (let i = 0; i < a.length; i++) out += (i ? sep : "") + String(a[i]);
  return out;
}
function trimmed(s: string): string { return s.trim(); }
function position(s: string): number { return s.indexOf("c") + 1; }
function code(s: string): number { let t = 0; for (let i = 0; i < s.length; i++) t += s.charCodeAt(i); return t; }
function first(s: string): string { return s.charAt(0); }
function contains(s: string): boolean { return s.includes("b"); }
function lower(s: string): string { return s.toLowerCase(); }
function joined(a: number[]): string { return a.join(","); }
function doubled(a: number[]): number { const b = a.map((x) => x * 2); return b[0] + b[1]; }
function kept(a: number[]): number { return a.filter((x) => x > 1).length; }
function summed(a: number[]): number { return a.reduce((p, x) => p + x, 0); }
function where(a: number[]): number { return a.indexOf(3) * 10; }
function added(a: number[]): number { return a.push(9); }
function scaled(t: Float64Array): number { const u = t.map((x) => x + 1); return u[0]; }
function round(): string {
  return cat([trimmed(" ab "), position("abc"), code("ab"), first("xy"), contains("abc"), lower("AB"),
              joined([1, 2]), doubled([1, 2]), kept([1, 2, 3]), summed([1, 2, 3]), where([1, 2, 3]),
              added([1]), scaled(new Float64Array([1, 2]))], "|");
}
const lines: string[] = [];
const note = (line: string) => { lines[lines.length] = line; };
for (let i = 0; i < 200; i++) round();
note("pristine " + round());
const S = String.prototype as any, A = Array.prototype as any, T = Float64Array.prototype as any;
const saved: any = {};
for (const k of ["trim", "indexOf", "charCodeAt", "charAt", "includes", "toLowerCase"]) saved["s" + k] = S[k];
for (const k of ["join", "map", "filter", "reduce", "indexOf", "push"]) saved["a" + k] = A[k];
S.trim = function () { return "<" + this + ">"; };
S.indexOf = function () { return "i"; };
S.charCodeAt = function () { return 1; };
S.charAt = function () { return "C"; };
S.includes = function () { return "yes"; };
S.toLowerCase = function () { return 7; };
A.join = function () { return "joined"; };
A.map = function () { return [10, 20]; };
A.filter = function () { return [0]; };
A.reduce = function () { return "R"; };
A.indexOf = function () { return 0.5; };
A.push = function () { return "pushed"; };
T.map = function () { return [42]; };
for (let i = 0; i < 200; i++) round();
note("patched  " + round());
for (const k of ["trim", "indexOf", "charCodeAt", "charAt", "includes", "toLowerCase"]) S[k] = saved["s" + k];
for (const k of ["join", "map", "filter", "reduce", "indexOf", "push"]) A[k] = saved["a" + k];
delete T.map;
for (let i = 0; i < 200; i++) round();
note("restored " + round());
for (let i = 0; i < lines.length; i++) console.log(lines[i]);
