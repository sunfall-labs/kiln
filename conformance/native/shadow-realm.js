// A ShadowRealm is a second set of intrinsics and a membrane around them. The C backend builds
// one global object, once, so it refuses this by name rather than answering with a realm whose
// intrinsics are secretly the only ones there are -- which would make every identity question
// below come out true when the language says false.
const realm = new ShadowRealm();

// A primitive crosses as itself.
console.log(realm.evaluate("1 + 1"));

// A callable crosses as a wrapped function belonging to *this* realm, not as itself.
const add = realm.evaluate("(function add(a, b) { return a + b; })");
console.log(typeof add, add.name, add.length, add(2, 3));
console.log(Object.getPrototypeOf(add) === Function.prototype);

// The wrapper shares no properties with what it wraps, in either direction.
const secretive = realm.evaluate("function f() { return f.secret; } f.secret = 'hidden'; f;");
console.log(secretive.secret, secretive());

// Nothing that is not a primitive or a callable crosses at all.
try {
  realm.evaluate("globalThis");
  console.log("LEAKED");
} catch (error) {
  console.log(error instanceof TypeError);
}

// Source that does not parse is a SyntaxError; source that parses and then throws is a
// TypeError, and the two must not be collapsed.
try {
  realm.evaluate("...");
} catch (error) {
  console.log(error.constructor.name);
}
try {
  realm.evaluate("throw new RangeError('x')");
} catch (error) {
  console.log(error.constructor.name);
}
