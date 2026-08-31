// A destructuring pattern usually declares bindings, and that is the easy half. The hard half is
// that its targets need not be bindings at all: a member, a computed member, a private field, or a
// binding that already exists. Each of those is an *assignment* performed in pattern position, so
// the pattern has to evaluate a reference rather than name a slot -- and the order in which it
// does is observable.

const target: any = {};
[target.first, target["second"]] = [1, 2];
console.log(target);
({ a: target.a, b: target["b"] } = { a: 3, b: 4 });
console.log(target);

// A computed key on both sides, evaluated where it is written.
const key = "chosen";
const computed: any = {};
({ [key]: computed[key] } = { chosen: 5 });
console.log(computed, computed.chosen);

// A private field is a target too, which is the case that cannot be spelled any other way: `#v` is
// not a property name and there is no `this["#v"]`.
class Box {
  #value = 0;
  fill(pair: number[]): number { [this.#value] = pair; return this.#value; }
  swap(source: any): number { ({ v: this.#value } = source); return this.#value; }
}
const box = new Box();
console.log(box.fill([7, 8]), box.swap({ v: 9 }));

// An element of an array, which is a member target whose key is a number.
const slots: any[] = [0, 0, 0];
[slots[2], slots[0]] = ["last", "first"];
console.log(slots);

// A binding that already exists, in a `for...of` and a `for...in` head. Both are the pattern
// assigning to something declared outside the loop, which is the spelling that made destructuring
// and iteration one feature rather than two.
let left: any;
let right: any;
for ([left, right] of [[1, 2], [3, 4]]) { console.log(left, right); }
let name: any;
const counts: any = { p: 1, q: 2 };
for (name in counts) { console.log(name, counts[name]); }
for ({ length: left } of ["ab", "cde"]) { console.log(left); }

// Defaults apply to undefined and to a missing element, and not to null -- the distinction the
// `??` operator was later named after.
const [first = 10, second = 20, third = 30] = [undefined, null];
console.log(first, second, third);
const { missing = "default", present = "default" } = { present: "given" } as any;
console.log(missing, present);

// A default is only evaluated when it is used, which is what makes an expensive one safe to write.
let evaluated = 0;
function expensive(): number { evaluated += 1; return evaluated; }
const { given = expensive(), absent = expensive() } = { given: "kept" } as any;
console.log(given, absent, evaluated);

// Nested, with a default for the whole nested pattern -- the idiom for an options object whose
// sections are themselves optional.
const { outer: { inner = "fallback" } = {} } = {} as any;
const { outer: { inner: reached = "fallback" } = {} } = { outer: { inner: "found" } } as any;
console.log(inner, reached);

// A rest element gathers what is left, and gathers nothing rather than failing when there is
// nothing left.
const [head, ...tail] = [1, 2, 3];
const [only, ...empty] = [1];
const { taken, ...rest } = { taken: 1, kept: 2, also: 3 } as any;
console.log(head, tail, only, empty, taken, rest);

// The right-hand side is walked with the iterator protocol, not by index, so a generator works and
// is asked for exactly as many values as the pattern has targets.
let produced = 0;
function* counted(): Generator<number> { while (true) { produced += 1; yield produced; } }
const [one, two] = counted();
console.log(one, two, produced);

// A pattern that stops early *closes* the iterator: the values it did not take are not simply
// abandoned, the source is told the walk is over. That is what lets an iterator holding a file
// handle or a lock release it, and it is the reason an array pattern is not just indexed reads.
let closed = 0;
const source: any = {
  [Symbol.iterator]() {
    let produced = 0;
    return {
      next() { produced += 1; return { value: produced, done: false }; },
      return() { closed += 1; return { done: true }; },
    };
  },
};
const [drawn] = source;
console.log(drawn, closed);
for (const value of source) { if (value === 2) { break; } }
console.log(closed);

// Swapping through a pattern, which is the case that proves the right-hand side is fully evaluated
// before anything is written.
let x = "x";
let y = "y";
[x, y] = [y, x];
console.log(x, y);

// A pattern in a parameter list, with a default for the whole parameter.
function draw({ width = 1, height = 2 } = {} as any): string { return `${width}x${height}`; }
console.log(draw(), draw({ width: 5 }), draw({ width: 5, height: 6 }));
