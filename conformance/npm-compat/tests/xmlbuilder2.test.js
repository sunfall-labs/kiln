// kiln eval smoke test (authored for kiln, not from porffor).
const {create} = require('xmlbuilder2');
console.log(create({root: {item: {'@id': 1, '#': 'text'}}}).end());
console.log(create({a: {b: 'x'}}).end({prettyPrint: true}).split('\n').length);
console.log(create().ele('root').ele('child').txt('hi').up().end());
console.log(create({list: {entry: [1, 2, 3]}}).end());
