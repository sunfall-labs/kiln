// An accessor is not a value that happens to be a function: reading one calls it, and writing
// through one calls a different one.
const counter = {
	_value: 1,
	get value() { return this._value * 10; },
	set value(next) { this._value = next; },
};
console.log(counter.value);
counter.value = 5;
console.log(counter.value, counter._value);

// Getter-only discards a write; setter-only reads as undefined.
const readOnly = { get fixed() { return 'fixed'; } };
readOnly.fixed = 'ignored';
console.log(readOnly.fixed);
const writeOnly = { set only(v) { this.seen = v; } };
console.log(String(writeOnly.only));
writeOnly.only = 'written';
console.log(writeOnly.seen);

// The two halves are written separately and describe one property.
const both = { get x() { return this.stored; }, set x(v) { this.stored = v * 2; } };
both.x = 21;
console.log(both.x);

// `this` inside an accessor is the object it was read from.
const owner = { name: 'owner', get label() { return 'I am ' + this.name; } };
console.log(owner.label);

// Class accessors live on the prototype, so they are inherited and are not own properties --
// which is why `JSON.stringify` and `Object.keys` do not see them.
class Temperature {
	constructor(celsius) { this.celsius = celsius; }
	get fahrenheit() { return this.celsius * 9 / 5 + 32; }
	set fahrenheit(value) { this.celsius = (value - 32) * 5 / 9; }
	static get unit() { return 'C'; }
}
const t = new Temperature(100);
console.log(t.fahrenheit, Temperature.unit);
t.fahrenheit = 32;
console.log(t.celsius, Object.keys(t).join(','), JSON.stringify(t));

// A subclass inherits them, and a setter found on the chain takes the write rather than being
// shadowed by a new own property.
class Kelvin extends Temperature {}
const k = new Kelvin(0);
k.fahrenheit = 212;
console.log(k.celsius, k.fahrenheit);

// An own accessor *is* serialised by JSON.stringify, which reads it.
console.log(JSON.stringify({ get computed() { return 1 + 1; }, plain: 3 }));

// A console names an accessor rather than calling it.
console.log({ get shown() { return 'not called'; }, set shown(v) {} });
console.log({ get onlyGet() { return 1; } });

// Spread reads the property, so it copies the value rather than the accessor.
const spread = { ...counter };
console.log(JSON.stringify(spread));

// A computed accessor key.
const key = 'dynamic';
const computed = { get [key]() { return 'from computed'; } };
console.log(computed.dynamic);
