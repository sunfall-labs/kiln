// A spread argument's length is not known until it runs, so the call's arity is no longer in the
// bytecode -- it is the length of a list the program computed.
function count(...args) { return args.length + ':' + args.join(','); }
const list = [2, 3];
console.log(count(...list), count(1, ...list), count(...list, 4), count(...list, ...list));
console.log(count(...[]), count());

// Every call shape: plain, method, computed method, and `new`.
const object = { name: 'o', join(...parts) { return this.name + '/' + parts.join('-'); } };
console.log(object.join(...list), object['join'](...list));
class Point { constructor(...values) { this.values = values.join(','); } }
console.log(new Point(...list).values, new Point(1, ...list).values);

// The receiver is evaluated once even with a spread, and before the arguments.
const order = [];
const receiver = () => { order.push('receiver'); return object; };
const spread = () => { order.push('spread'); return list; };
receiver().join(...spread());
console.log(order.join(','));

// Any iterable spreads, not only an array.
function* pair() { yield 'a'; yield 'b'; }
console.log(count(...pair()), count(...new Set([1, 1, 2])), count(...'hi'));

// Math.max is the classic reason to want this at all.
console.log(Math.max(...[1, 5, 3]), Math.min(...[1, 5, 3]));
