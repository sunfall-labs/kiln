// JSON, both ways. Serialising can *call* -- `toJSON` runs before anything else is decided about
// a value, and a property may be an accessor -- so the serialiser takes a host and a depth like
// every other read does.
console.log(JSON.stringify({ a: 1, b: "x", c: true, d: null, e: [1, 2] }));
console.log(JSON.stringify([1, "a", true, null, undefined]));
console.log(JSON.stringify({ n: 1 }), JSON.stringify(undefined), JSON.stringify(1), JSON.stringify("s"));

// Values with no JSON spelling: dropped from an object, null in an array, because an array's
// positions are part of its meaning.
console.log(JSON.stringify({ u: undefined, n: 1 }), JSON.stringify([undefined, 1]));
console.log(JSON.stringify(1 / 0), JSON.stringify(NaN), JSON.stringify(-0), JSON.stringify(""));

// Escapes, including the C0 controls that have no shorthand.
console.log(JSON.stringify("he said \"hi\"\n\t\\"), JSON.stringify("a\u0001b"));
console.log(JSON.stringify({ "key with space": 1, "0": 2, "a-b": 3 }));

// Indentation, by count and by string, and the empty cases that take none.
console.log(JSON.stringify({ a: { b: { c: 1 } } }, null, 2));
console.log(JSON.stringify([1, [2, [3]]], null, "  "));
console.log(JSON.stringify({}, null, 2), JSON.stringify([], null, 2), JSON.stringify({ a: 1 }, null, 4));

// `toJSON` runs first, at every level, and its result is serialised in turn.
console.log(JSON.stringify({ t: { toJSON() { return "custom"; } } }));
class Money { cents: number; constructor(c: number) { this.cents = c; } toJSON(): string { return `$${this.cents / 100}`; } }
console.log(JSON.stringify({ price: new Money(1250) }), JSON.stringify({ m: new Money(500) }, null, 2));

// A getter is read, which is a call, and what it produced is what gets serialised.
const computed = { base: 2, get doubled(): number { return this.base * 2; } };
console.log(JSON.stringify(computed));

// Parsing: every shape, whitespace anywhere, and escapes on the way back in.
console.log(JSON.parse('{"a":1,"b":[true,null,"x"]}'), JSON.parse("3.5"), JSON.parse('"s"'));
console.log(JSON.parse("null"), JSON.parse("true"), JSON.parse("-1.5e3"), JSON.parse("[]"), JSON.parse("{}"));
console.log(JSON.parse(' { "a" : [ 1 , 2 ] } '), JSON.parse('"\\u0041\\n"').length, JSON.parse('"\\ud83d\\ude00"'));

// The round trip, which is the property a program actually depends on.
const original = { name: "kiln", tags: ["a"], nested: { n: 1 }, none: null };
console.log(JSON.parse(JSON.stringify(original)));
console.log(JSON.stringify(JSON.parse(JSON.stringify(original))) === JSON.stringify(original));
