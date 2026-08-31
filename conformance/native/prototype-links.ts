// Four ways to set a prototype, and they do not agree about what to do with a value that cannot be
// one. `{ __proto__: v }` ignores it, the `__proto__` setter ignores it, `Object.setPrototypeOf`
// throws, and `Object.create` throws -- the specification draws each line deliberately, and a
// backend that treated them alike would leave an object linked to a number, unable to find
// `toString` through a chain that no longer ends anywhere.

const parent = { greet(): string { return "hello"; } };

// The literal production, which is the only spelling of `__proto__` that links rather than stores.
const linked: any = { __proto__: parent, own: 1 };
console.log(linked.greet(), Object.getPrototypeOf(linked) === parent, Object.keys(linked));

// A computed key is an ordinary property called `__proto__`, because the specification asks
// whether the key was written out rather than what it evaluated to. So is a shorthand.
const stored: any = { ["__proto__"]: parent };
console.log(Object.getPrototypeOf(stored) === Object.prototype, Object.keys(stored), stored.__proto__ === parent);

// A primitive is ignored: the literal is an object with no property and its ordinary prototype,
// not an object linked to a number.
const ignored: any = { __proto__: 42, own: 2 };
console.log(Object.getPrototypeOf(ignored) === Object.prototype, Object.keys(ignored), ignored.toString !== undefined);
const alsoIgnored: any = { __proto__: undefined, own: 3 };
console.log(Object.getPrototypeOf(alsoIgnored) === Object.prototype, alsoIgnored.own);

// Null is not a primitive here: it is the one value that means "no chain at all", and it is what
// `Object.create(null)` asks for by another spelling.
const bare: any = { __proto__: null, own: 4 };
console.log(Object.getPrototypeOf(bare), Object.keys(bare), Object.getPrototypeOf(Object.create(null)));

// The accessor pair on `Object.prototype`: reading it reports the same link `getPrototypeOf` does,
// and writing it relinks the object rather than giving it a property of that name.
const moving: any = {};
console.log(moving.__proto__ === Object.prototype, Object.keys(moving));
moving.__proto__ = parent;
console.log(moving.greet(), Object.getPrototypeOf(moving) === parent, Object.keys(moving), Object.hasOwn(moving, "__proto__"));
moving.__proto__ = 7;
console.log(Object.getPrototypeOf(moving) === parent, Object.keys(moving));

// `Object.setPrototypeOf` is the spelling that complains, because a program that asked for a
// prototype in so many words is better told the request could not be met.
const explicit: any = {};
try { Object.setPrototypeOf(explicit, 42); console.log("accepted a number"); } catch (error: any) { console.log(error.name); }
console.log(Object.getPrototypeOf(explicit) === Object.prototype);
console.log(Object.setPrototypeOf(explicit, parent) === explicit, explicit.greet());
console.log(Object.setPrototypeOf(explicit, null) === explicit, Object.getPrototypeOf(explicit));

// A relinked object is still the object it was: identity does not follow the chain.
const relinked: any = { own: 5 };
const before = relinked;
Object.setPrototypeOf(relinked, parent);
console.log(before === relinked, relinked.own, relinked.greet());

// `Object.create` builds the same shape from the other end, and the chain it makes is walked by
// reads and by `instanceof` alike.
const grandparent = { deep(): string { return "deep"; } };
const middle = Object.create(grandparent);
const leaf = Object.create(middle);
console.log(leaf.deep(), Object.getPrototypeOf(leaf) === middle, Object.getPrototypeOf(middle) === grandparent);
console.log(Object.hasOwn(leaf, "deep"), "deep" in leaf, leaf.deep === grandparent.deep);

// A class's chain, seen from both ends: an instance links to the prototype, and the prototype
// carries `constructor` back -- which is a real property and is never enumerated.
class Animal { speak(): string { return "..."; } }
class Dog extends Animal { speak(): string { return "woof"; } }
const dog = new Dog();
console.log(dog.speak(), dog instanceof Dog, dog instanceof Animal, Object.getPrototypeOf(dog) === Dog.prototype);
console.log(Object.getPrototypeOf(Dog.prototype) === Animal.prototype, Dog.prototype.constructor === Dog);
console.log(Object.keys(Dog.prototype), Object.hasOwn(Dog.prototype, "constructor"));
