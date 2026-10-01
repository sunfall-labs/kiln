// kiln eval smoke test (authored for kiln, not from porffor).
import {merge} from 'merge-anything';
console.log(JSON.stringify(merge({a: {x: 1}}, {a: {y: 2}, b: 3})));
console.log(JSON.stringify(merge({list: [1, 2]}, {list: [3]})));
console.log(JSON.stringify(merge({}, {a: 1})));
console.log(JSON.stringify(merge({a: 1}, {a: 2})));
console.log(JSON.stringify(merge({a: {b: {c: 1}}}, {a: {b: {d: 2}}})));
