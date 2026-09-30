// kiln eval smoke test (authored for kiln, not from porffor).
import { minimatch } from 'minimatch';

console.log(minimatch('a.js', '*.js'));
console.log(minimatch('a/b.js', 'a/*.js'));
console.log(minimatch('foo.ts', '*.{js,ts}'));
console.log(minimatch('a/b/c.js', 'a/**/*.js'));
console.log(minimatch('B.JS', '*.js', { nocase: true }));
