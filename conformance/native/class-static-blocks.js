// A `static { }` block is a body run once where the class is defined, with `this` bound to the
// constructor. Both engines compile the same bytecode for it -- a function made and applied to
// the class -- so what this probe is really asking is whether `Apply` with a computed receiver
// means the same thing in each.
class Counter {
  static seen = [];
  static first = Counter.seen.push("field-1");
  static {
    // `this` is the constructor, and a `var` declared here stays here.
    var scratch = "block-1";
    this.seen.push(scratch);
  }
  static second = Counter.seen.push("field-2");
  static #hidden = 41;
  static {
    // The class's private names are in scope, and so is the class binding itself.
    Counter.seen.push("private=" + (Counter.#hidden + 1));
  }
}
console.log(Counter.seen.join(","));
console.log(typeof scratch);

class Base {
  static label() {
    return "base";
  }
}
class Derived extends Base {
  static {
    // `super` reads through the constructor's home object, as it does in a static method.
    console.log(super.label(), this === Derived);
  }
}

// A block that throws propagates out of the class definition, and the binding never appears.
try {
  class Doomed {
    static {
      throw new TypeError("from the block");
    }
  }
} catch (error) {
  console.log(error.constructor.name + ": " + error.message);
}
console.log(typeof Doomed);
