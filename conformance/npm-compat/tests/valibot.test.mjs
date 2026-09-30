// kiln eval smoke test (authored for kiln, not from porffor).
import * as v from 'valibot';

const User = v.object({ name: v.string(), age: v.optional(v.number()) });
console.log(JSON.stringify(v.parse(User, { name: 'Ann', age: 25 })));

const bad = v.safeParse(User, { name: 42 });
console.log(bad.success === false);

const Email = v.pipe(v.string(), v.email());
console.log(v.is(Email, 'a@b.com'));
console.log(v.is(Email, 'not-an-email'));

const Pair = v.tuple([v.string(), v.number()]);
console.log(JSON.stringify(v.parse(Pair, ['x', 1])));
