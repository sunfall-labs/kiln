// `new`, prototypes, and the chain a property read walks. A method lives on the constructor's
// prototype object, so finding it is the chain working; `instanceof` reads the same chain from
// the other end, which is why the two have to agree about its shape.
function Point(this: any, x: number, y: number) { this.x = x; this.y = y; }
Point.prototype.sum = function (this: any): number { return this.x + this.y; };
const p = new Point(3, 4);
console.log(p.x, p.y, p.sum(), p instanceof Point, p.constructor === Point);

class Counter {
  n: number;
  constructor(start: number) { this.n = start; }
  bump(): number { this.n += 1; return this.n; }
  read(): number { return this.n; }
}
const c = new Counter(10);
console.log(c.bump(), c.bump(), c.read(), c instanceof Counter, typeof Counter);

// A constructor that returns an object: the returned object wins over the instance, which is
// what makes a factory constructor work.
function Factory(this: any) { this.ignored = 1; return { made: true }; }
const made = new Factory();
console.log(made, made.ignored, made instanceof Factory);

// Two instances share the prototype's method and not each other's own properties.
const a = new Counter(0);
const b = new Counter(100);
a.bump();
console.log(a.n, b.n, a.bump === b.bump, Object.hasOwn(a, "n"), Object.hasOwn(a, "bump"));

// A function carries properties of its own, separately from what it constructs.
function Tagged(this: any) { this.kind = "instance"; }
Tagged.kind = "static";
console.log(new Tagged().kind, Tagged.kind, Tagged.name, typeof Tagged.prototype);

// The chain is walked, not just the own properties: a name found two links up still resolves.
const base = { greet(): string { return "hi"; } };
const middle = Object.create(base);
const leaf = Object.create(middle);
console.log(leaf.greet(), Object.hasOwn(leaf, "greet"), base.greet === leaf.greet);

// `instanceof` against something that never constructed the value.
function Other(this: any) {}
console.log(p instanceof Other, ({}) instanceof Point, [] instanceof Point);

// Methods reached through the chain and used as values.
console.log(c.read(), [1, 2].map((n: number) => new Counter(n).read()));
