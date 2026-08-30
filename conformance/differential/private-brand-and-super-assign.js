// `#x in o` is the brand check: does this object carry the class's private field?
class Counter {
  #count = 0;
  static holds(value) {
    return #count in value;
  }
  bump() {
    this.#count += 1;
    return this.#count;
  }
}
class Lookalike {
  #count = 0;
}
const counter = new Counter();
console.log(Counter.holds(counter), Counter.holds(new Lookalike()));
console.log(Counter.holds({}), Counter.holds(null === null ? {} : 1));
console.log(counter.bump(), Counter.holds(counter));

// A private method is a brand too.
class Tagged {
  #mark() {
    return "marked";
  }
  static is(value) {
    return #mark in value;
  }
}
console.log(Tagged.is(new Tagged()), Tagged.is(counter));

// `super.x = v` writes to the instance, and a setter on the parent takes it.
class Base {
  constructor() {
    this.log = [];
  }
  set watched(value) {
    this.log.push("base:" + value);
  }
}
class Derived extends Base {
  write(value) {
    super.watched = value;
    super.plain = value;
    return this.plain;
  }
}
const derived = new Derived();
console.log(derived.write(7), derived.log.join(","));
console.log(Object.prototype.hasOwnProperty.call(derived, "plain"));
