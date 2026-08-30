// A constructor and its prototype point at each other, which is what `x.constructor` reads.
class Point {
  constructor(x) {
    this.x = x;
  }
}
const p = new Point(1);
console.log(p.constructor === Point, Point.prototype.constructor === Point);
console.log(p.constructor.name, Point.name);

// The link survives inheritance: a subclass instance names the subclass.
class Point3 extends Point {
  constructor(x) {
    super(x);
    this.z = 0;
  }
}
console.log(new Point3(1).constructor === Point3, new Point3(1).constructor.name);

// A console names the class an instance came from, and leaves a plain object bare.
console.log(p);
console.log(new Point3(2));
console.log({ x: 1 });
console.log([p]);

// An ordinary function used as a constructor works the same way.
function Legacy(v) {
  this.v = v;
}
console.log(new Legacy(3).constructor === Legacy);
console.log(new Legacy(3));

// A prototype replaced wholesale loses the link, exactly as it does in the language.
function Bare() {}
Bare.prototype = {};
console.log(new Bare().constructor === Bare);
