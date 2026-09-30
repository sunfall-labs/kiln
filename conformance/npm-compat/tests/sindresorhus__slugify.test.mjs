// kiln eval smoke test (authored for kiln, not from porffor).
import slugify from '@sindresorhus/slugify';
console.log(slugify('Hello World!'));
console.log(slugify('  Déjà vu  '));
console.log(slugify('foo_bar baz', {separator: '_'}));
console.log(slugify('a/b\\c'));
console.log(slugify('UPPER lower'));
