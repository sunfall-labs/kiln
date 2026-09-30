// kiln eval smoke test (authored for kiln, not from porffor).
const striptags = require('striptags');

console.log(striptags('<p>Hello <b>world</b></p>'));
console.log(striptags('<p>Hello <b>world</b></p>', ['b']));
console.log(striptags('<a href="x">link</a> text'));
console.log(striptags('no tags here'));
console.log(striptags('<div><span>nested</span></div>', [], '\n'));
