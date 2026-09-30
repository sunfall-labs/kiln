// kiln eval smoke test (authored for kiln, not from porffor).
import {map, filter, pipe, reduce, take} from 'rambda';
console.log(JSON.stringify(map(x => x * 2, [1, 2, 3])));
console.log(JSON.stringify(filter(x => x > 1, [1, 2, 3])));
console.log(JSON.stringify(pipe([1, 2, 3, 4], filter(x => x % 2 === 0), map(x => x + 1))));
console.log(reduce((a, b) => a + b, 0, [1, 2, 3]));
console.log(JSON.stringify(take(2, [1, 2, 3])));
