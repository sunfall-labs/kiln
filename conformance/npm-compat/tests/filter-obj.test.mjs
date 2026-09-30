// kiln eval smoke test (authored for kiln, not from porffor).
import {includeKeys, excludeKeys} from 'filter-obj';
console.log(JSON.stringify(includeKeys({a: 1, b: 2, c: 3}, ['a', 'c'])));
console.log(JSON.stringify(excludeKeys({a: 1, b: 2, c: 3}, ['b'])));
console.log(JSON.stringify(includeKeys({a: 1, b: 2}, [])));
console.log(JSON.stringify(excludeKeys({a: 1, b: 2}, ['zzz'])));
console.log(JSON.stringify(includeKeys({x: 1, y: 2, z: 3}, ['y'])));
