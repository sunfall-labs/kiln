// kiln eval smoke test (authored for kiln, not from porffor).
import arrayUnion from 'array-union';

console.log('basic:', JSON.stringify(arrayUnion([1, 2], [2, 3], [3, 4])));
console.log('strings:', JSON.stringify(arrayUnion(['a', 'b'], ['b', 'c'])));
console.log('empty:', JSON.stringify(arrayUnion([], [])));
console.log('single:', JSON.stringify(arrayUnion([1, 1, 1])));
console.log('order preserved:', JSON.stringify(arrayUnion([3, 1], [2, 1])));
console.log('mixed types:', JSON.stringify(arrayUnion([1, 'a'], ['a', true])));
