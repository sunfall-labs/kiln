// kiln eval smoke test (authored for kiln, not from porffor).
const {unique, sum, group, range, zip, cluster, sift, alphabetical} = require('radash');
console.log(JSON.stringify(unique([1, 2, 2, 3])));
console.log(sum([1, 2, 3]));
console.log(JSON.stringify(group([1, 2, 3, 4], x => x % 2)));
console.log(JSON.stringify(range(0, 5)));
console.log(JSON.stringify(zip([1, 2], ['a', 'b'])));
console.log(JSON.stringify(cluster([1, 2, 3, 4, 5], 2)));
console.log(JSON.stringify(sift([0, 1, false, 2, '', 3])));
console.log(JSON.stringify(alphabetical([{n: 'b'}, {n: 'a'}], x => x.n).map(x => x.n)));
