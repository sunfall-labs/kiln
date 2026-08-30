// A method call is not a property read followed by a call: that would discard the receiver, and
// the read has to yield the unbound function so `a.f === a.f` holds. Both have to be true.
const counter = {
  n: 0,
  bump(): number { this.n += 1; return this.n; },
  read(): number { return this.n; },
};
console.log(counter.bump(), counter.bump(), counter.read(), counter.n);
console.log(counter.read === counter.read, typeof counter.bump);

// `this` is the receiver of the call, not the object the method was written in.
const other = { n: 100, read: counter.read };
console.log(other.read(), counter.read());

// A module's `this` is undefined: a module is strict, and strict mode does not substitute the
// global object.
console.log(this);

// A plain call passes undefined too, so a function that was a method reads `this` as undefined
// -- which is why it has to be called through the object to work.
function standalone(a: number, b: number): number { return a + b; }
const holder = { go: standalone, value: 7 };
console.log(holder.go(1, 2), standalone(1, 2));

// Methods that build and return objects, and a method reached through a nested object.
const factory = {
  make(n: number) { return { n, twice: n * 2 }; },
};
const outer = { inner: factory };
console.log(factory.make(4), outer.inner.make(5).twice);

// Recursion through a method, and a closure created inside one.
const tree = {
  depth(n: number): number { return n <= 0 ? 0 : 1 + tree.depth(n - 1); },
  adder(by: number) { return (x: number): number => x + by; },
};
console.log(tree.depth(6), tree.adder(10)(5));
