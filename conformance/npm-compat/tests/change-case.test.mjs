// kiln eval smoke test (authored for kiln, not from porffor).
import {camelCase, snakeCase, pascalCase, kebabCase, capitalCase, constantCase} from 'change-case';
console.log(camelCase('foo bar'));
console.log(snakeCase('fooBar'));
console.log(pascalCase('foo-bar'));
console.log(kebabCase('fooBar'));
console.log(capitalCase('foo bar'));
console.log(constantCase('fooBar'));
