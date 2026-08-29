// A class is a constructor function whose `prototype` holds the instance methods. That is not a
// simplification -- it is what a class is -- so classes reuse the machinery `new` already needs.
class Counter {
  constructor(start: number) {
    this.value = start;
  }

  increment(): number {
    this.value += 1;
    return this.value;
  }

  static describe(): string {
    return "a counter";
  }
}

const counter = new Counter(10);
console.log("field", counter.value);
console.log("method", counter.increment(), counter.increment());
// Statics live on the constructor, not on the prototype.
console.log("static", Counter.describe());

// Instances are independent: a write finds an own property rather than the shared prototype.
const other = new Counter(0);
console.log("independent", counter.value, other.value);

// A class with no constructor gets a synthesized empty one.
class Empty {}
console.log("empty", typeof new Empty());

// `this` comes from the receiver for a method, and from the defining scope for an arrow.
const holder = {
  label: "held",
  read() {
    const lexical = () => this.label;
    return lexical();
  },
};
console.log("arrow this", holder.read());

// A constructor returning a primitive still yields the instance; returning an object overrides.
function Odd() {
  this.kind = "instance";
  return 1;
}
console.log("primitive return ignored", new Odd().kind);
