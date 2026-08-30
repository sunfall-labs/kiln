// `this` in a derived constructor is the result of `super()`, not an ordinary object with the
// parent's properties copied onto it. That is the whole of subclassing a built-in: an array
// subclass instance *is* an array.
class Stack extends Array {
	peek() { return this[this.length - 1]; }
}
const stack = new Stack();
stack.push(1, 2);
console.log(stack instanceof Array, stack instanceof Stack, Array.isArray(stack));
console.log(stack.length, stack.peek(), stack.join(','), JSON.stringify(stack));
console.log(stack.map((n) => n * 2).join(','));

// The same for Map and Set.
class Registry extends Map {
	getOr(key, fallback) { return this.has(key) ? this.get(key) : fallback; }
}
const registry = new Registry();
registry.set('k', 1);
console.log(registry instanceof Map, registry instanceof Registry, registry.get('k'), registry.size);
console.log(registry.getOr('k', 0), registry.getOr('missing', 'fallback'));

class Tags extends Set {}
const tags = new Tags();
tags.add('a'); tags.add('a'); tags.add('b');
console.log(tags instanceof Set, tags.size, [...tags].join(','));

// A Set is not a Map and vice versa.
console.log(new Set() instanceof Map, new Map() instanceof Set);

// Error subclassing, which used to need its own special case and now goes through the same path.
class HttpError extends Error {
	constructor(status) { super('status ' + status); this.status = status; }
}
const failure = new HttpError(404);
console.log(failure instanceof Error, failure instanceof HttpError, failure.message, failure.status);
console.log(failure.name, String(failure));

// `new Array` keeps its old ambiguity: one number is a length, anything else is the elements.
console.log(new Array(3).length, new Array('3').length, new Array(1, 2).join(','));
console.log(JSON.stringify(new Array(2)));
