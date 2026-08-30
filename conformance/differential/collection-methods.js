// `sort` is in place, stable, and compares by *string* unless given a comparator -- which is why
// [10, 9, 1] sorts to [1, 10, 9] and surprises everyone once.
const numbers = [10, 9, 1, 2];
console.log(numbers.sort().join(','), numbers.sort((a, b) => a - b).join(','));
const original = [3, 1];
console.log(original.sort() === original);

// Stability: equal keys keep their original order.
const records = [
	{ k: 1, tag: 'a' }, { k: 0, tag: 'b' }, { k: 1, tag: 'c' }, { k: 0, tag: 'd' },
];
console.log(records.sort((x, y) => x.k - y.k).map((r) => r.tag).join(''));

// Undefined sorts to the end and never reaches the comparator.
let compared = 0;
const holes = [3, undefined, 1];
console.log(holes.sort((a, b) => { compared++; return a - b; }).map(String).join(','), compared);

// The rest of the array surface these probes reach for.
const list = [1, 2, 3];
console.log(list.at(0), list.at(-1), list.at(9), list.concat([4], 5).join(','));
console.log([1, 2, 3].reverse().join(','), [1, 2].findIndex((n) => n === 2), [1, 2].findIndex((n) => n === 9));
const queue = [1, 2, 3];
console.log(queue.shift(), queue.unshift(0), queue.join(','), [1, 2, 1].lastIndexOf(1));
console.log([1, 2, 3, 4].fill(0, 1, 3).join(','), [1, 2, 3].reduce((a, b) => a + b), [1, 2].reduce((a, b) => a + b, 10));

// Strings.
const text = 'abcdef';
console.log(text.at(-1), text.charAt(1), text.charCodeAt(0), text.lastIndexOf('c'));
console.log('5'.padStart(3, '0'), 'x'.padEnd(4, 'ab'), 'y'.padStart(0), 'z'.padEnd(3));
console.log('a-b-c'.replace('-', '+'), 'a-b-c'.replaceAll('-', '+'), '  x  '.trimStart() + '|', '  x  '.trimEnd() + '|');
console.log('a'.concat('b', 1, null));
