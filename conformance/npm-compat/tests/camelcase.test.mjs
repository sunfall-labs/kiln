// kiln eval smoke test (authored for kiln, not from porffor).
import camelCase from 'camelcase';
console.log(camelCase('foo-bar'));
console.log(camelCase('foo bar'));
console.log(camelCase('--foo.bar'));
console.log(camelCase('Foo-Bar'));
console.log(camelCase('foo_bar_baz'));
console.log(camelCase('foo-bar', { pascalCase: true }));
console.log(camelCase(''));
