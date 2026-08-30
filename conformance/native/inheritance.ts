// `class D extends C` links two pairs of things: the prototype objects, so an instance of `D`
// finds a method of `C`, and the *constructors*, so `D` finds a static of `C`. Missing the second
// is why `class B extends A {}` refused before -- it is not the same link.
class Animal {
  name: string;
  constructor(name: string) { this.name = name; }
  speak(): string { return `${this.name} makes a sound`; }
  describe(): string { return `<${this.speak()}>`; }
  static kingdom(): string { return "Animalia"; }
}
class Dog extends Animal {
  constructor(name: string) { super(name); }
  speak(): string { return `${this.name} barks`; }
}
class Puppy extends Dog {}

const dog = new Dog("Rex");
console.log(dog.name, dog.speak(), dog.describe());
console.log(dog instanceof Dog, dog instanceof Animal, dog instanceof Puppy);

// A derived class with no constructor forwards whatever it was given, and its arity is not known
// where the forwarding is synthesized -- so it comes from the frame at run time.
const puppy = new Puppy("Spot");
console.log(puppy.name, puppy.speak(), puppy instanceof Puppy, puppy instanceof Animal);

// An overridden method is found before the inherited one, and the inherited one still calls
// through `this` -- which is what makes `describe` report the *subclass* sound.
console.log(new Animal("Generic").describe(), puppy.describe());

// A static is inherited through the constructor link.
console.log(Dog.kingdom(), Puppy.kingdom(), Animal.kingdom());

// `super()` runs the parent against the instance that already exists rather than building one,
// which is the whole difference between `super()` and `new Parent()`.
class Counted extends Animal {
  // Declared without a type annotation on purpose: kiln follows the class-fields semantics
  // where a declared field is initialised at construction, and node's type stripping erases the
  // declaration instead -- so an annotated field would put `count` before `name` here and after
  // it there. That is a TypeScript question rather than an engine one.
  constructor(name: string) {
    super(name);
    this.count = this.name.length;
  }
}
const counted = new Counted("Fido");
console.log(counted.name, counted.count, counted.speak(), Object.keys(counted));

// Three levels, with each adding a field.
class Base { a = 1; }
class Middle extends Base { b = 2; }
class Leaf extends Middle { c = 3; }
const leaf = new Leaf();
console.log(leaf.a, leaf.b, leaf.c, leaf instanceof Base, Object.keys(leaf));

// A method reached through the chain and used as a value.
console.log([new Dog("A"), new Puppy("B")].map((animal: Animal) => animal.speak()));
