import { bench } from "../harness.mjs";

class Point {
  x: number;
  y: number;
  constructor(x: number, y: number) {
    this.x = x;
    this.y = y;
  }
  dist(): number {
    return Math.sqrt(this.x * this.x + this.y * this.y);
  }
}

class Point3D extends Point {
  z: number;
  constructor(x: number, y: number, z: number) {
    super(x, y);
    this.z = z;
  }
  dist(): number {
    return Math.sqrt(this.x * this.x + this.y * this.y + this.z * this.z);
  }
}

const pts: Point[] = [];
for (let i: number = 0; i < 1000; i++) {
  pts.push(i % 2 === 0 ? new Point(i, i + 1) : new Point3D(i, i + 1, i + 2));
}

bench("class-poly", (): number => {
  let sum: number = 0;
  for (let r: number = 0; r < 500; r++) {
    for (let i: number = 0; i < pts.length; i++) {
      sum += pts[i].dist();
    }
  }
  return Math.round(sum);
});
