// `accessor x = v` is three members written as one: a private slot nothing can name, and the
// `get x` / `set x` pair that reads and writes it. Both engines compile the same bytecode for
// the desugaring, so what this probe asks is whether a private field plus an accessor pair mean
// the same thing in each -- including on the constructor, where the slot belongs to the class
// that declared it and a subclass reaching for the inherited getter finds nothing.
class Counter {
  accessor untouched;
  accessor value = 1;
  accessor "quoted" = 2;
  accessor 3 = 4;
  static accessor shared = 5;
}
const counter = new Counter();
console.log(counter.untouched, counter.value, counter.quoted, counter[3], Counter.shared);
counter.value = 10;
Counter.shared = 11;
console.log(counter.value, Counter.shared);

// The backing slot is private, so nothing reflective can see it and the public name is an
// accessor rather than a data property.
const described = Object.getOwnPropertyDescriptor(Counter.prototype, "value");
console.log(
  typeof described.get,
  typeof described.set,
  described.enumerable,
  described.configurable,
  "value" in described,
  Object.getOwnPropertyNames(counter).length,
  described.get.name,
  described.set.name,
);

// A subclass inherits the getter, and the getter finds no slot on the subclass constructor.
class Derived extends Counter {}
try {
  console.log("derived", Derived.shared);
} catch (error) {
  console.log("derived", error.constructor.name);
}

// A getter or setter written beside the accessor overrides exactly the half it declares, because
// the accessor's pair is installed where the source wrote it and no earlier.
class Overridden {
  #hidden = 1;
  get value() {
    return this.#hidden;
  }
  accessor value = 2;
  set value(written) {
    this.#hidden = written;
  }
  hidden() {
    return this.#hidden;
  }
}
const overridden = new Overridden();
overridden.value = 3;
console.log("overridden", overridden.value, overridden.hidden());
