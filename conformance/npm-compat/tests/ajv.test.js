// kiln eval smoke test (authored for kiln, not from porffor).
const Ajv = require('ajv');
const ajv = new Ajv();
const validate = ajv.compile({
  type: 'object',
  properties: {name: {type: 'string'}, age: {type: 'integer', minimum: 0}},
  required: ['name'],
  additionalProperties: false,
});
console.log(validate({name: 'Ada', age: 36}));
console.log(validate({name: 42}));
console.log(validate({name: 'Ada', extra: true}) ? 'valid' : validate.errors[0].keyword);
const v2 = ajv.compile({type: 'array', items: {type: 'number'}, minItems: 2});
console.log(v2([1, 2, 3]));
console.log(v2([1]) ? 'valid' : v2.errors[0].keyword);
