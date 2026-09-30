// kiln eval smoke test (authored for kiln, not from porffor).
const slugify = require('slugify');

console.log(slugify('Hello World'));
console.log(slugify('Hello World', { lower: true }));
console.log(slugify('Café & Restaurant!', { lower: true, strict: true }));
console.log(slugify('hello_world', '_'));
console.log(slugify.extend({ '☃': 'snowman' }) && slugify('a ☃ b', { lower: true }));
