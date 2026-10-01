import * as v from 'valibot';

console.log('--- Object validation with defaults and transforms ---');
const UserSchema = v.object({
  id: v.pipe(v.number(), v.minValue(1)),
  username: v.pipe(v.string(), v.trim(), v.toLowerCase(), v.minLength(3)),
  role: v.optional(v.picklist(['admin', 'member', 'guest']), 'guest'),
  tags: v.optional(v.array(v.string()), []),
  metadata: v.optional(v.record(v.string(), v.unknown()), {})
});

const user1 = v.parse(UserSchema, {
  id: 10,
  username: '  AliceAdmin  ',
  tags: ['lead', 'core']
});
console.log(JSON.stringify(user1));

console.log('--- safeParse failure & issue extraction ---');
const badUser = v.safeParse(UserSchema, {
  id: -5,
  username: '  a  ',
  role: 'superman'
});
console.log('badUser success:', badUser.success);
if (!badUser.success) {
  const flat = v.flatten(badUser.issues);
  console.log('nested issue keys:', Object.keys(flat.nested).sort());
}

console.log('--- Email pipe validation ---');
const Email = v.pipe(v.string(), v.email());
console.log(v.is(Email, 'a@b.com'));
console.log(v.is(Email, 'not-an-email'));

console.log('--- Discriminated Unions / Variants ---');
const ActionSchema = v.variant('type', [
  v.object({ type: v.literal('create'), name: v.string() }),
  v.object({ type: v.literal('delete'), id: v.number() })
]);

console.log(JSON.stringify(v.parse(ActionSchema, { type: 'create', name: 'ItemA' })));
console.log(JSON.stringify(v.parse(ActionSchema, { type: 'delete', id: 42 })));
console.log('invalid variant:', v.safeParse(ActionSchema, { type: 'unknown' }).success);

console.log('--- Pipe Transformations & Coercion ---');
const NumericStringSchema = v.pipe(
  v.string(),
  v.trim(),
  v.transform((val) => Number(val)),
  v.pipe(v.number(), v.minValue(0))
);
console.log(v.parse(NumericStringSchema, '  123.45  '));

console.log('--- Tuple validation ---');
const CoordSchema = v.tuple([v.number(), v.number(), v.optional(v.number(), 0)]);
console.log(JSON.stringify(v.parse(CoordSchema, [10, 20])));
console.log(JSON.stringify(v.parse(CoordSchema, [10, 20, 30])));
