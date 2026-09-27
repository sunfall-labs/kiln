class Point { constructor(x, y) { this.x = x; this.y = y; } dist() { return Math.sqrt(this.x * this.x + this.y * this.y); } }
class Point3D extends Point { constructor(x, y, z) { super(x, y); this.z = z; } dist() { return Math.sqrt(this.x * this.x + this.y * this.y + this.z * this.z); } }
const pts = [];
for (let i = 0; i < 1000; i++) pts.push(i % 2 === 0 ? new Point(i, i + 1) : new Point3D(i, i + 1, i + 2));
function work() { let sum = 0; for (let r = 0; r < 500; r++) for (let i = 0; i < pts.length; i++) sum += pts[i].dist(); return Math.round(sum); }
work();
const t0 = Date.now(); let s = 0; for (let k = 0; k < 20; k++) s += work(); console.log(s, Date.now() - t0, "ms");
