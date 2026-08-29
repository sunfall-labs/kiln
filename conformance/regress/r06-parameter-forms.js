// Every parameter form. The default was the reason this probe exists: oxc keeps a parameter's
// default on the FormalParameter rather than inside its binding pattern, so a frontend that reads
// only the pattern sees a plain identifier for `f(x = 1)` and drops the default without a word --
// a silently altered function rather than a refused one.
function withDefault(x = 5) { return x; }
// A default applies to `undefined` only, not to every falsy value and not to null.
console.log(withDefault(), withDefault(2), withDefault(undefined), withDefault(null));

// A default may read an earlier parameter, and is evaluated per call rather than once.
const arrowDefault = (a, b = a * 2) => a + b;
function fresh(list = []) { list.push(1); return list.length; }
console.log(arrowDefault(1), arrowDefault(1, 10), fresh(), fresh());

// Rest is an array even when nothing is left over, and `arguments` still reports what was passed
// rather than what was defaulted.
function rest(first, ...others) { return first + ':' + others.join(','); }
function counted(x = 5) { return arguments.length; }
function empty(a, ...more) { return Array.isArray(more) + ':' + more.length; }
console.log(rest(1), rest(1, 2, 3), counted(), counted(1), empty(1));

// Destructuring, with renaming, holes, defaults inside the pattern, nesting, and rest.
const object = ({ x, y: renamed, z = 9 }) => [x, renamed, z].join(',');
const array = ([first, , third = 7]) => [first, third].join(',');
function nested({ a: { b } }, [c]) { return b + c; }
function restInPattern({ keep, ...others }) { return keep + ' ' + JSON.stringify(others); }
console.log(object({ x: 1, y: 2 }), array([1, 2]), nested({ a: { b: 10 } }, [5]), restInPattern({ keep: 1, extra: 2 }));

// A closure over a parameter shares the binding, including one written in another parameter's
// default -- where nothing in the function *body* reveals that the parameter is captured.
function counter(start = 0) { return () => ++start; }
function late(a, cb = () => a) { a = 9; return cb(); }
function destructured({ value }) { return () => value; }
const next = counter();
console.log(next(), next(), late(1), destructured({ value: 42 })());

// A destructured parameter shadows an outer binding of the same name.
const value = 'outer';
function shadow({ value }) { return value; }
console.log(shadow({ value: 'inner' }), value);

class Point {
	constructor({ x = 0, y = 0 } = {}) { this.x = x; this.y = y; }
	moved(...deltas) { return deltas.length; }
}
const point = new Point({ x: 3 });
console.log(point.x, point.y, point.moved(1, 2));
