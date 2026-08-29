// `instanceof` walks the prototype chain looking for the constructor's prototype by identity.
class Animal {}
class Dog extends Animal {}

const dog = new Dog();
console.log("own class", dog instanceof Dog);
console.log("through the chain", dog instanceof Animal);
console.log("not a descendant", new Animal() instanceof Dog);
console.log("plain object", ({}) instanceof Animal, "primitive", 1 instanceof Animal);

// Errors participate: `Error.prototype` is one object per run, so a subclass instance is
// recognised by the constructor it never directly mentions.
class Failure extends Error {}
const failure = new Failure("went wrong");
console.log("error subclass", failure instanceof Failure, failure instanceof Error);
// A derived class with no constructor forwards its arguments, so the message survives.
console.log("forwarded message", failure.message);
console.log("rendered", String(failure));

// Prototypes can also be set directly, which is how a constructor gains inherited statics
// without `extends`.
const shared = { tag: "shared" };
function Plain() {}
Object.setPrototypeOf(Plain, shared);
console.log("inherited static", Plain.tag, Object.getPrototypeOf(Plain) === shared);
