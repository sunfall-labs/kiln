// An arrow nested in a derived constructor inherits lexical `this` and `super`, but its return
// value is still the arrow's own. It must not be validated as the enclosing constructor's return.
class Base {
  constructor(value) {
    this.value = value;
  }
}

class Derived extends Base {
  constructor(values) {
    super(values.map((value) => value + 1));
    this.missing = values.map(() => {}).join(",");
  }
}

const derived = new Derived([1, 2, 3]);
console.log(derived.value.join(","), derived.missing);
