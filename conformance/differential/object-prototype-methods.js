// The `Object.prototype` methods every object inherits, and the type test built on one of them.
const plain = { a: 1 };
console.log(plain.hasOwnProperty("a"), plain.hasOwnProperty("b"));
console.log(Object.prototype.hasOwnProperty.call(plain, "a"));
console.log(plain.propertyIsEnumerable("a"));

// `Object.prototype.toString.call` is the oldest type test there is, and still the one bundles
// reach for.
const tag = (value) => Object.prototype.toString.call(value);
console.log(tag([]), tag({}), tag(1), tag("s"), tag(true));
console.log(tag(null), tag(undefined), tag(/x/), tag(new Date(0)));

// `isPrototypeOf` walks the chain, so a grandparent answers true.
class A {}
class B extends A {}
const b = new B();
console.log(A.prototype.isPrototypeOf(b), B.prototype.isPrototypeOf(b));
console.log(Object.getPrototypeOf(b) === B.prototype);

// A method read off a prototype table is unbound, so the receiver comes from `call`.
console.log(Array.prototype.indexOf.call([7, 8, 9], 8));
console.log(String.prototype.toUpperCase.call("shout"));
console.log(Number.prototype.toFixed.call(1.5, 2));
