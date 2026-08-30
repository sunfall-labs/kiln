// Private members, and the three things every callable answers to.
//
// A private name is not a string key: it is found on the object itself or nowhere, and "nowhere"
// is a TypeError. That own-only rule *is* the brand check.

class Counter {
  #count = 0;
  static #created = 0;
  #bump(): number { this.#count += 1; return this.#count; }
  static #record(): number { Counter.#created += 1; return Counter.#created; }
  constructor() { Counter.#record(); }
  next(): number { return this.#bump(); }
  get count(): number { return this.#count; }
  static get created(): number { return Counter.#created; }
}
const counter = new Counter();
console.log(counter.next(), counter.next(), counter.count, Counter.created);

// Nothing that enumerates own string properties can see a private name, and `#x` and `"#x"` name
// different properties.
console.log(Object.keys(counter).join(",") || "(none)", JSON.stringify(counter));
console.log(JSON.stringify({ ...counter }), Object.entries(counter).length);
const shaped = { "#count": "a string key" };
console.log(shaped["#count"], Object.keys(shaped).join(","));

// Not inherited: a subclass that never ran the declaring class's installation does not carry it.
class Base { static probe(): string { return this.#secret; } static #secret = "base"; }
class Derived extends Base {}
console.log(Base.probe());
try { Derived.probe(); console.log("no throw"); } catch (error: any) { console.log("threw", error.constructor.name); }

// ...but an instance private *is* reachable from an inherited method, because the base
// constructor ran and installed it on this very object.
class Animal { #sound = "noise"; speak(): string { return this.#sound; } }
class Dog extends Animal {}
console.log(new Dog().speak(), new Counter().count);

// `call`, `apply` and `bind` on every kind of callable: a compiled function, a runtime-supplied
// method, a constructor, and something already bound.
function add(this: { base: number }, a: number, b: number): number { return this.base + a + b; }
console.log(add.call({ base: 1 }, 2, 3), add.apply({ base: 1 }, [2, 3]));
const bound = add.bind({ base: 10 });
console.log(bound(2, 3), add.bind({ base: 10 }, 2)(3), add.bind({ base: 10 }, 2, 3)());
console.log(bound.call({ base: 99 }, 2, 3), typeof bound.bind({}));
console.log(Object.prototype.hasOwnProperty.call({ a: 1 }, "a"));
console.log(Object.prototype.hasOwnProperty.call({ a: 1 }, "b"));
console.log(Array.prototype.join.call([1, 2, 3], "-"), String.prototype.trim.call("  x  "));
// An assigned property wins, exactly as an own property shadows an inherited one.
function shadowed(): void {}
(shadowed as any).call = 1;
console.log((shadowed as any).call, typeof add.call);

// `new f(...args)` is the same construction the fixed-arity form performs.
class Point {
  x: number;
  y: number;
  constructor(x: number, y: number) { this.x = x; this.y = y; }
  sum(): number { return this.x + this.y; }
}
const parts: [number, number] = [3, 4];
console.log(new Point(...parts).sum(), new Point(...([1, 2] as [number, number])).x);
console.log(new Date(...([2020, 0, 15] as [number, number, number])).getUTCFullYear());

// A method reached by a computed key runs against the object it was reached through, which is why
// `obj[name]()` is one instruction rather than a read followed by a call.
const api: any = { greet(name: string) { return `hi ${name} from ${this.tag}`; }, tag: "api" };
const key = "greet";
console.log(api[key]("a"), api["greet"]("b"), api[`gr${"eet"}`]("c"));
const table: any = { "+": (l: number, r: number) => l + r, "-": (l: number, r: number) => l - r };
for (const op of ["+", "-"]) { console.log(op, table[op](7, 2)); }

// A compound computed assignment reads the object and the key once and uses them twice, which two
// duplications cannot do because they would reverse them.
const counts: any = { a: 1 };
counts["a"] += 5;
counts["a"] *= 2;
counts["b"] ??= 7;
counts["b"] ||= 9;
console.log(JSON.stringify(counts));
const nested: any = { list: [1, 2, 3] };
nested["list"][0] += 10;
console.log(nested.list.join(","), nested["list"]["join"](","));
// The key expression runs once, which is what "read once" means when it has a side effect.
let reads = 0;
const keys = ["a", "b"];
const pick = (): string => { reads += 1; return keys[0]; };
counts[pick()] += 1;
console.log(reads, counts.a);
