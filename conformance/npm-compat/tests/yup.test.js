const yup = require('yup');

console.log('--- Nested Object & Defaults & Transforms ---');
const userSchema = yup.object({
  username: yup.string().trim().lowercase().min(3).required(),
  email: yup.string().email().required(),
  age: yup.number().positive().integer().optional(),
  role: yup.string().oneOf(['admin', 'member', 'guest']).default('guest'),
  active: yup.boolean().default(true),
  tags: yup.array().of(yup.string().lowercase()).default([]),
  meta: yup.object({
    loginCount: yup.number().default(0)
  }).default({})
});

const user1 = userSchema.validateSync({
  username: '  AdminBob  ',
  email: 'bob@example.com',
  tags: ['DEV', 'Ops']
});
console.log(JSON.stringify(user1));

console.log('--- Conditional validation with when ---');
const orderSchema = yup.object({
  isGift: yup.boolean().default(false),
  giftMessage: yup.string().when('isGift', {
    is: true,
    then: (s) => s.required(),
    otherwise: (s) => s.strip()
  })
});
console.log(JSON.stringify(orderSchema.validateSync({ isGift: true, giftMessage: 'Happy Birthday!' })));
console.log(JSON.stringify(orderSchema.validateSync({ isGift: false, giftMessage: 'Ignored' })));

console.log('--- Detailed multi-error collection ---');
try {
  userSchema.validateSync({
    username: 'a',
    email: 'valid@example.com',
    age: -10
  }, { abortEarly: false });
} catch (e) {
  const paths = e.inner.map(err => err.path).sort();
  console.log('error paths:', paths);
}

console.log('--- Tuple & Cast ---');
const tupleSchema = yup.tuple([yup.string(), yup.number()]);
console.log(JSON.stringify(tupleSchema.validateSync(['key', 42])));
console.log(JSON.stringify(yup.array(yup.number()).cast(['10', '20.5', 30])));
console.log('valid email:', yup.string().email().isValidSync('test@kiln.dev'));
