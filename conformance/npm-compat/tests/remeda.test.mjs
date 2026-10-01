// kiln eval smoke test (authored for kiln, not from porffor).
import {map, filter, pipe, reduce, groupBy, unique} from 'remeda';
console.log(JSON.stringify(pipe([1, 2, 3, 4], filter(x => x % 2 === 0), map(x => x * 10))));
console.log(reduce([1, 2, 3], (acc, x) => acc + x, 0));
console.log(JSON.stringify(groupBy(['a', 'bb', 'ccc'], x => x.length)));
console.log(JSON.stringify(unique([3, 1, 2, 1, 3])));
console.log(JSON.stringify(map([1, 2, 3], x => x + 1)));
