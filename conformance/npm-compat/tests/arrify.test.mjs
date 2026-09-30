// kiln eval smoke test (authored for kiln, not from porffor).
import arrify from 'arrify';

console.log('value:', JSON.stringify(arrify('x')));
console.log('array passthrough:', JSON.stringify(arrify([1, 2])));
console.log('null:', JSON.stringify(arrify(null)));
console.log('undefined:', JSON.stringify(arrify(undefined)));
console.log('number:', JSON.stringify(arrify(0)));
console.log('object:', JSON.stringify(arrify({ a: 1 })));
console.log('nested array:', JSON.stringify(arrify([[1], [2]])));
