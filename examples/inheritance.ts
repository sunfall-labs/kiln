// `extends` is two prototype links, not a special object model: instance methods are inherited
// through the prototypes, static methods through the constructors themselves.
class Animal {
  constructor(name: string) {
    this.name = name;
  }
  speak(): string {
    return this.name + " makes a sound";
  }
  static kind(): string {
    return "animal";
  }
}

class Dog extends Animal {
  constructor(name: string) {
    // The parent initialises the instance being built rather than allocating its own.
    super(name);
    this.legs = 4;
  }
  speak(): string {
    // `super.m()` resolves on the parent's prototype but runs against the current receiver.
    return super.speak() + " (a bark)";
  }
}

const dog = new Dog("Rex");
console.log("inherited field", dog.name, "own field", dog.legs);
console.log("override calls super", dog.speak());
console.log("parent unaffected", new Animal("Cat").speak());
console.log("static inherited", Dog.kind());

// A native parent builds its own object, so its properties are copied onto the instance. That
// is what makes `extends Error` produce a usable error rather than an empty one.
class TaggedError extends Error {
  constructor(message: string) {
    super(message);
    this.tag = "tagged";
  }
}
const failure = new TaggedError("boom");
console.log("error subclass", failure.message, failure.tag, typeof failure.stack);
