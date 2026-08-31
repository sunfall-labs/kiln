// What a function says about itself before it is called: its name and its length. Both are read by
// libraries -- a dependency injector reads `length` to decide how many arguments to supply, and a
// test runner reads it to tell a callback that wants a `done` from one that does not -- so getting
// either wrong is wrong in a way that only shows up in someone else's code.

// `length` is how many arguments a caller *must* supply, which is the parameters before the first
// default and before a rest. It is not how many slots the frame has, and the difference is exactly
// the optional ones.
function required(a: number, b: number): number { return a + b; }
const defaulted = (a: number, b = 2): number => a + b;
const resting = (a: number, ...more: number[]): number => a + more.length;
const onlyRest = (...all: number[]): number => all.length;
const afterDefault = (a: number, b = 2, c = 3): number => a + b + c;
console.log(required.length, defaulted.length, resting.length, onlyRest.length, afterDefault.length);
console.log((() => {}).length, ((a: any) => a).length, function (a: any, b: any) {}.length);

// A destructuring parameter still counts as one, and a default on it still stops the count.
const pattern = ({ x }: any, [y]: any): any => [x, y];
const patternDefault = ({ x }: any = {}, y?: any): any => [x, y];
console.log(pattern.length, patternDefault.length);

// `name` comes from where the function was written, not from what it was later assigned to -- a
// named expression keeps its own name, and an anonymous one takes the binding's.
function declared(): void {}
const inferred = function (): void {};
const alsoInferred = (): void => {};
const named = function itsOwnName(): void {};
console.log(declared.name, inferred.name, alsoInferred.name, named.name);
console.log((function () {}).name, (() => {}).name);

// A method takes the key it was written under, including a computed one; an accessor and a class
// are named too.
const holder = { method(a: number) {}, arrow: (a: number) => a, ["computed"](a: number, b: number) {} };
console.log(holder.method.name, holder.arrow.name, holder.computed.name, holder.computed.length);
// A computed method key and a class's own methods are named "" in both engines where node names
// them "computed", "make" and "area". That is the pair being wrong together -- a gap in how a
// method's name is recorded, not a backend disagreement -- so it is recorded here rather than
// hidden: this corpus asks whether the two engines agree, and `conformance/differential` is where
// the same lines are measured against node. See NOTES.md.
class Shape { static make() {} area(): number { return 0; } get size(): number { return 1; } }
console.log(Shape.name, Shape.length, Shape.make.name, Shape.prototype.area.name);
class WithFields { constructor(a: number, b: number) {} }
console.log(WithFields.name, WithFields.length);

// `bind` reports what is still to be supplied and prefixes the name it wrapped, which is what
// makes a bound function readable in a stack trace.
const bound = required.bind(null, 1);
const boundBoth = required.bind(null, 1, 2);
console.log(bound.name, bound.length, boundBoth.name, boundBoth.length, bound(2), boundBoth());

// The built-ins answer the same two questions.
console.log(Object.name, Array.name, Math.max.name, [].map.name, "".slice.name);
console.log(typeof Object.prototype, typeof Array.prototype, Array.prototype.constructor === Array);

// A function is an object: it carries properties of its own, separately from what it constructs,
// and those are enumerated where `name`, `length` and `prototype` are not.
function tagged(): void {}
(tagged as any).version = 2;
console.log(Object.keys(tagged), (tagged as any).version, Object.hasOwn(tagged, "name"));
