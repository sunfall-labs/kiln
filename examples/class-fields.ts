// Field initialisers run inside the constructor, after `super()` and before its body. That
// order is not a convenience: a field may read `this`, and in a derived class `this` does not
// exist until the parent has run.
class Point {
  x = 0;
  y = 0;
  label; // no initialiser: the property exists and is undefined, rather than being absent
  static origin = "0,0";

  constructor(x: number) {
    this.x = x; // runs after the field defaults, so it wins
  }

  describe(): string {
    return this.x + "," + this.y;
  }
}

const point = new Point(5);
console.log("fields", point.x, point.y, point.label, point.describe());
console.log("static field", Point.origin);
console.log("independent", point.x, new Point(1).x);

// A field initialiser is evaluated in the class's enclosing scope, so it can close over a
// variable the class was defined next to -- including from a class expression returned by a
// function, where each call produces a class closing over that call's binding.
const makeTagged = (tag: string) => {
  class Tagged {
    tag = tag;
    read = () => tag;
  }
  return Tagged;
};

const First = makeTagged("first");
const Second = makeTagged("second");
console.log("captured by field", new First().tag, new First().read());
console.log("separate captures", new Second().tag, new First().tag);
