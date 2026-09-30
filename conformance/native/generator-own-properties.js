function* values() { yield 3; }
const iter = values();
const token = Symbol('token');
iter.stream = { label: 'source' };
iter[token] = 'symbol';
Object.defineProperty(iter, 'hidden', { value: 7, configurable: true });
console.log(iter.stream.label, 'stream' in iter, Object.hasOwn(iter, 'stream'));
console.log(Object.keys(iter).join(','), Reflect.ownKeys(iter).length, iter[token]);
console.log(Object.getOwnPropertyDescriptor(iter, 'stream').enumerable);
console.log(Object.assign({}, iter).stream.label, Reflect.set(iter, 'extra', 9), iter.extra);
console.log(iter.next().value, typeof iter.return);
console.log(delete iter.stream, 'stream' in iter, Object.keys(iter).join(','));
async function* asyncValues() { yield 'async'; }
const asyncIter = asyncValues();
asyncIter.stream = { label: 'async source' };
console.log(asyncIter.stream.label, Object.hasOwn(asyncIter, 'stream'));
