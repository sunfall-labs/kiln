// kiln eval smoke test (authored for kiln, not from porffor).
import {diff, jsonPatchPathConverter} from 'just-diff';
console.log(JSON.stringify(diff({a: 1, b: 2}, {a: 9, b: 2})));
console.log(JSON.stringify(diff([1, 2], [1, 2, 3])));
console.log(JSON.stringify(diff({x: 1}, {})));
console.log(typeof jsonPatchPathConverter);
console.log(JSON.stringify(jsonPatchPathConverter(['a', 'b'])));
