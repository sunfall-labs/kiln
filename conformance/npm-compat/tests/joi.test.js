// kiln eval smoke test (authored for kiln, not from porffor).
const Joi = require('joi');
const schema = Joi.object({
  name: Joi.string().min(3).required(),
  age: Joi.number().integer().min(0).default(0),
  tags: Joi.array().items(Joi.string()),
});
const good = schema.validate({ name: 'Ada', age: 36, tags: ['x'] });
console.log(good.error, JSON.stringify(good.value));
const bad = schema.validate({ name: 'Al' });
console.log(bad.error ? bad.error.message : 'valid');
console.log(Joi.number().validate('not a number').error.message.split('\n')[0]);
console.log(JSON.stringify(schema.validate({ name: 'Bob' }).value));
