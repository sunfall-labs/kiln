// `delete` removes an own property and evaluates to whether the property is gone afterwards --
// which is why deleting something that was never there is still true.
const target = { a: 1, b: 2 };
console.log(delete target.a, JSON.stringify(target), delete target.nothing);
const key = 'b';
console.log(delete target[key], JSON.stringify(target));
// The receiver is evaluated once, even when the property is computed.
const calls = [];
const receiver = () => { calls.push('called'); return { x: 1, y: 2 }; };
console.log(delete receiver().x, calls.length);
// Deleting an element leaves a hole rather than shortening the array.
const list = [1, 2, 3];
console.log(delete list[1], list.length, String(list[1]), list.join(','));
// Deleting from a primitive is harmless and still reports true.
console.log(delete 'text'.length, delete (5).x);
