// `new.target` names the constructor a `new` call used, and is undefined in a plain call.
function Shape(kind) {
  if (new.target === undefined) {
    return "called";
  }
  this.kind = kind;
  return undefined;
}
console.log(Shape("x"));
console.log(new Shape("circle").kind, new Shape("c") instanceof Shape);

// It names the constructor that was *invoked*, which is how a base class tells which subclass
// is being built.
class Base {
  constructor() {
    this.builtBy = new.target.name;
  }
}
class Derived extends Base {}
console.log(new Base().builtBy, new Derived().builtBy);

// An arrow has no `new.target` of its own and uses the one from where it was written.
function Outer() {
  const inner = () => (new.target === Outer ? "outer" : String(new.target));
  return inner();
}
console.log(new Outer(), Outer());

// A method is an ordinary call, however it was reached.
class Holder {
  check() {
    return new.target === undefined;
  }
}
console.log(new Holder().check());
