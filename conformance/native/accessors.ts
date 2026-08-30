// Reading a property can be a *call*, which is why every program-visible read goes through the
// runtime rather than a struct field access -- and why the read carries a depth and somewhere to
// put a throw, exactly as a call does.
const person = {
  first: "Ada",
  last: "Lovelace",
  get full(): string { return `${this.first} ${this.last}`; },
  set full(value: string) { this.first = value; },
};
console.log(person.full);
person.full = "Grace";
console.log(person.full, person.first, person);

// A getter with no setter swallows the write rather than shadowing it, which is the difference
// between a silent no-op and a silently wrong object.
const readOnly = { get fixed(): number { return 7; } };
readOnly.fixed = 99;
console.log(readOnly.fixed, Object.hasOwn(readOnly, "fixed"), readOnly);

// An accessor on a class lives on the prototype, so the write walks the chain to find it.
class Temperature {
  celsius = 0;
  get fahrenheit(): number { return this.celsius * 9 / 5 + 32; }
  set fahrenheit(value: number) { this.celsius = (value - 32) * 5 / 9; }
  get isFreezing(): boolean { return this.celsius <= 0; }
}
const t = new Temperature();
t.fahrenheit = 212;
console.log(t.celsius, t.fahrenheit, t.isFreezing);
t.fahrenheit = 32;
console.log(t.celsius, t.isFreezing, Object.hasOwn(t, "fahrenheit"), Object.hasOwn(t, "celsius"));

// The receiver is the object the read started from, not the one the accessor was found on.
const proto = { get who(): string { return this.name; } };
const child = Object.create(proto);
child.name = "child";
console.log(child.who);

// Two accessors over one backing field, and a getter that calls a method.
const account = {
  cents: 0,
  get dollars(): number { return this.cents / 100; },
  set dollars(value: number) { this.cents = Math.round(value * 100); },
  describe(): string { return `$${this.dollars}`; },
  get summary(): string { return this.describe(); },
};
account.dollars = 12.34;
console.log(account.cents, account.dollars, account.summary);

// A getter that throws is caught like any other call.
const bad = { get boom(): number { throw "from a getter"; } };
try { console.log(bad.boom); } catch (e) { console.log("caught", e); }

// A console names an accessor rather than calling it: printing must not run the program.
let reads = 0;
const counted = { get watched(): number { reads += 1; return reads; } };
console.log(counted, reads, counted.watched, reads);
