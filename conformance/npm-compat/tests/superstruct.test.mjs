import {
  object,
  string,
  number,
  boolean,
  array,
  optional,
  enums,
  size,
  pattern,
  defaulted,
  assign,
  refine,
  create,
  mask,
  assert,
  is,
  validate
} from 'superstruct';

console.log('--- Struct Composition & Defaults ---');
const BaseEntity = object({
  id: number(),
  active: defaulted(boolean(), true)
});

const Role = enums(['admin', 'editor', 'viewer']);
const UserProfile = assign(
  BaseEntity,
  object({
    username: size(string(), 3, 20),
    email: pattern(string(), /^[\w.-]+@[\w.-]+\.\w+$/),
    role: defaulted(Role, 'viewer'),
    tags: optional(array(string()))
  })
);

const created = create({
  id: 1,
  username: 'antigravity',
  email: 'test@example.com'
}, UserProfile);
console.log(JSON.stringify(created));

console.log('--- Masking extra fields ---');
const LooseUser = object({
  id: number(),
  name: string()
});
const rawInput = { id: 42, name: 'Alice', secretKey: 'dontleak', extra: 123 };
const masked = mask(rawInput, LooseUser);
console.log(JSON.stringify(masked));

console.log('--- Custom Refinement ---');
const EvenNumber = refine(number(), 'even', (v) => v % 2 === 0);
console.log('is 4 even:', is(4, EvenNumber));
console.log('is 5 even:', is(5, EvenNumber));

console.log('--- Validation and Error Details ---');
const [err, val] = validate({
  id: 'not-a-number',
  username: 'ab',
  email: 'bad-email'
}, UserProfile);
console.log('has error:', err !== undefined);
if (err) {
  const failurePaths = err.failures().map(f => f.path.join('.')).sort();
  console.log('failures:', failurePaths);
}

console.log('--- Assert & Throw ---');
try {
  assert({ id: 2, username: 'bob', email: 'valid@example.com' }, UserProfile);
  console.log('assert succeeded');
} catch (e) {
  console.log('unexpected throw');
}

try {
  assert({ id: 2, username: 'x' }, UserProfile);
  console.log('unexpected pass');
} catch (e) {
  console.log('assert failed correctly:', e.name === 'StructError');
}
