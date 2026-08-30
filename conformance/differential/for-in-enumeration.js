// `for-in` walks own enumerable string keys, in enumeration order -- indices first and ascending,
// then insertion order.
const object = { b: 1, a: 2, 1: 3, 0: 4 };
const seen = [];
for (const key in object) seen.push(key);
console.log(seen.join(','));

// The values are looked up through the key, and the key is a string even for an index.
const pairs = [];
for (const key in object) pairs.push(key + '=' + object[key] + ':' + typeof key);
console.log(pairs.join(' '));

// A key deleted by the body is not visited; one added may or may not be, and is not here.
const mutating = { a: 1, b: 2, c: 3 };
const visited = [];
for (const key in mutating) {
	visited.push(key);
	delete mutating.c;
	mutating.d = 4;
}
console.log(visited.join(','));

// Class methods live on the prototype and are not enumerable, so an instance shows only fields.
class Point {
	constructor() { this.x = 1; this.y = 2; }
	move() {}
}
const fields = [];
for (const key in new Point()) fields.push(key);
console.log(fields.join(','));

// An array enumerates its indices as strings; a string enumerates its character indices.
const indices = [];
for (const index in ['a', 'b']) indices.push(index + ':' + typeof index);
console.log(indices.join(','));

// `break` and `continue` work, and a closure made in the body captures that step's key.
const closures = [];
const kept = [];
for (const key in object) {
	if (key === '1') continue;
	if (key === 'b') break;
	closures.push(() => key);
	kept.push(key);
}
console.log(kept.join(','), closures.map((f) => f()).join(','));

// Enumerating nothing runs the body zero times.
let ran = 0;
for (const key in {}) ran++;
for (const key in null) ran++;
for (const key in undefined) ran++;
console.log(ran);
