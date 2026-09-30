// kiln eval smoke test (authored for kiln, not from porffor).
import { object, string, number, array, optional, assert, is, validate } from 'superstruct';

const User = object({ name: string(), age: optional(number()) });
assert({ name: 'Ann', age: 25 }, User);
console.log(is({ name: 'Ann' }, User));

const [err, value] = validate({ name: 42 }, User);
console.log(err !== undefined, value === undefined);

const List = array(number());
console.log(is([1, 2, 3], List));
console.log(is(['a'], List));

try {
  assert({ name: 'x', age: 'old' }, User);
  console.log('no-throw');
} catch (e) {
  console.log(e.name, e.failures().length > 0);
}
