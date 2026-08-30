// A private name is not a string key: it is found on the object itself or nowhere, and "nowhere"
// is a TypeError. That own-only rule *is* the brand check -- an object of another class does not
// carry the name however much of the shape it shares.
class Counter {
	#count = 0;
	static #created = 0;
	#bump() { this.#count += 1; return this.#count; }
	static #record() { Counter.#created += 1; return Counter.#created; }
	constructor() { Counter.#record(); }
	next() { return this.#bump(); }
	get count() { return this.#count; }
	static get created() { return Counter.#created; }
}
const counter = new Counter();
console.log(counter.next(), counter.next(), counter.count, Counter.created);

// Private names are not own enumerable string properties, so nothing that enumerates sees them.
console.log(Object.keys(counter).join(',') || '(none)', JSON.stringify(counter));

// `#x` and `"#x"` name different properties.
const shaped = { '#count': 'a string key' };
console.log(shaped['#count'], Object.keys(shaped).join(','));

// A private name is not inherited: a subclass constructor that never ran the declaring class's
// installation does not carry it. Reaching a static private through a subclass is the classic
// case, and it throws.
class Base { static probe() { return this.#secret; } static #secret = 'base'; }
class Derived extends Base {}
console.log(Base.probe());
try { Derived.probe(); console.log('no throw'); } catch (e) { console.log('threw', e.constructor.name); }

// An instance private *is* reachable from an inherited method, because the base constructor ran
// and installed it on this very object.
class Animal { #sound = 'noise'; speak() { return this.#sound; } }
class Dog extends Animal {}
console.log(new Dog().speak());

// Two instances have their own copies.
const first = new Counter();
const second = new Counter();
first.next();
console.log(first.count, second.count);
