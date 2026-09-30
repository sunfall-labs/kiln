// kiln eval smoke test (authored for kiln, not from porffor).
const Ajv = require('ajv');
const addFormats = require('ajv-formats');
const ajv = new Ajv();
addFormats(ajv);
const v = ajv.compile({type: 'string', format: 'email'});
console.log(v('ada@example.com'), v('not-an-email'));
const vd = ajv.compile({type: 'string', format: 'date'});
console.log(vd('2024-01-31'), vd('not-a-date'));
const vu = ajv.compile({type: 'string', format: 'uri'});
console.log(vu('https://example.com/x'), vu('::not a uri::'));
