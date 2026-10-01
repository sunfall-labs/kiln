// kiln eval smoke test (authored for kiln, not from porffor).
import {z} from 'zod';
const User = z.object({name: z.string(), age: z.number().int().min(0)});
console.log(JSON.stringify(User.parse({name: 'Ada', age: 36})));
console.log(User.safeParse({name: 42}).success);
console.log(z.string().email().safeParse('not-an-email').success);
console.log(JSON.stringify(z.array(z.number()).parse([1, 2, 3])));
console.log(z.union([z.string(), z.number()]).parse(7));
console.log(z.coerce.number().parse('42'));
