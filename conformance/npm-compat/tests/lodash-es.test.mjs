// kiln eval smoke test (authored for kiln, not from porffor).
import chunk from 'lodash-es/chunk.js';
import uniq from 'lodash-es/uniq.js';
import groupBy from 'lodash-es/groupBy.js';
import sortBy from 'lodash-es/sortBy.js';

console.log(JSON.stringify(chunk([1, 2, 3, 4, 5], 2)));
console.log(JSON.stringify(uniq([1, 2, 2, 3, 1])));
console.log(JSON.stringify(groupBy([1.1, 2.2, 1.9], Math.floor)));
console.log(JSON.stringify(sortBy([{ n: 2 }, { n: 1 }], 'n')));
