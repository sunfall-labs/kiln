// kiln eval smoke test (authored for kiln, not from porffor).
const yup = require('yup');
const schema = yup.object({name: yup.string().required(), age: yup.number().min(0)});
console.log(JSON.stringify(schema.validateSync({name: 'Ada', age: 36})));
console.log(schema.isValidSync({name: 'Ada'}));
try {
  schema.validateSync({name: '', age: -1}, {abortEarly: false});
} catch (e) {
  console.log('errors:', e.errors.length);
}
console.log(yup.string().email().isValidSync('a@b.co'));
console.log(JSON.stringify(yup.array(yup.number()).cast(['1', '2'])));
