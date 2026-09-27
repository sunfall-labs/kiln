// kiln-native: profile=native-release
//
// Realms and concurrency issue 20, for `scripts/native-realms.sh`: typed bodies that inline
// `Math.sqrt` run a long stretch against the pristine built-ins and only then does the program
// patch the member. Realms of one process run it side by side and reach the patch at different
// times, so one realm's store happens while another is still in its pristine stretch: each must
// see only its own. `Math.foo` is written first, which must change nothing.
Math.foo = 1;
class Point { constructor(x, y) { this.x = x; this.y = y; }
               dist() { return Math.sqrt(this.x * this.x + this.y * this.y); } }
function hyp(a, b) { return Math.sqrt(a * a + b * b); }
const pts = []; for (let i = 0; i < 2000; i++) pts.push(new Point(i % 97, (i * 7) % 89));
function measure() {
  let s = 0;
  for (let round = 0; round < 200; round++) {
    for (const p of pts) s += p.dist();
    s += hyp(round, round + 1);
  }
  return s;
}
const before = measure();
const original = Math.sqrt;
Math.sqrt = (x) => x / 2;
const during = measure();
Math.sqrt = original;
const after = measure();
console.log(before, during, after, before === after);
