// kiln eval smoke test (authored for kiln, not from porffor).
const classNames = require('classnames');
console.log(classNames('a', 'b'));
console.log(classNames('a', {b: true, c: false}));
console.log(classNames(['a', 'b'], 'c'));
console.log(classNames({x: 1}, null, undefined, 0, 'y'));
console.log(JSON.stringify(classNames()));
console.log(classNames('a', ['b', {c: true}]));
