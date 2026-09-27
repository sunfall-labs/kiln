class P { constructor(x, y) { this.x = x; this.y = y; } dist() { return this.x + this.y; } }
class Q { constructor(x, y) { this.x = x; this.y = y; } dist() { return this.x * this.y; } }
const pts = [];
for (let i = 0; i < 100; i++) pts.push(i % 2 ? new P(i, 2) : new Q(i, 2));
function run() {
  let sum = 0;
  for (let r = 0; r < 3; r++) for (let i = 0; i < pts.length; i++) sum += pts[i].dist();
  return sum;
}
console.log(run(), run());
