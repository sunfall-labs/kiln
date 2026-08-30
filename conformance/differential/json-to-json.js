// `toJSON` runs before anything else is decided about a value: it is how a Date serialises as its
// ISO string and how a library controls its own JSON form. kiln ignored it, so any object that
// defined one serialised as its raw fields instead -- a wrong answer with no diagnostic, and one
// that only shows up in whatever consumes the JSON.
console.log(JSON.stringify({ t: { toJSON() { return "custom"; } } }));
console.log(JSON.stringify({ toJSON() { return { replaced: true }; } }));
console.log(JSON.stringify([{ toJSON() { return 1; } }, { toJSON() { return undefined; } }]));

// Looked up on the value, prototype chain included, so a class method counts.
class Money {
  constructor(cents) { this.cents = cents; }
  toJSON() { return `$${this.cents / 100}`; }
}
console.log(JSON.stringify({ price: new Money(1250) }), JSON.stringify(new Money(1)));

// A `toJSON` that is not callable is ignored rather than being an error.
console.log(JSON.stringify({ toJSON: 5, a: 1 }), JSON.stringify({ toJSON: null, a: 2 }));

// The result of `toJSON` is serialised in turn, nesting and all.
console.log(JSON.stringify({ toJSON() { return { inner: { toJSON() { return [1, 2]; } } }; } }));

// A Date carries one, which is the whole reason the step exists.
console.log(JSON.stringify(new Date(0)), JSON.stringify({ at: new Date(86400000) }));

// It runs under indentation too, and its result is what gets indented.
console.log(JSON.stringify({ m: new Money(500) }, null, 2));

// `toJSON` and `toISOString` are not the same function, and the difference is exactly the case
// JSON cares about: an instant with no ISO spelling makes `toISOString` throw, where `toJSON`
// answers null so the enclosing document still serialises.
const invalid = new Date(NaN);
console.log(JSON.stringify({ when: invalid }), JSON.stringify(invalid), invalid.toJSON());
try { invalid.toISOString(); } catch (e) { console.log(e instanceof RangeError); }
