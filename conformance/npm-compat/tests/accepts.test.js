// kiln eval smoke test (authored for kiln, not from porffor).
const accepts = require('accepts');
const a = accepts({headers: {accept: 'text/html, application/json;q=0.9'}});
console.log(a.type(['json', 'html']));
console.log(JSON.stringify(a.types()));
console.log(a.type('json'));
const b = accepts({headers: {'accept-language': 'en-US,en;q=0.9'}});
console.log(b.language(['es', 'en']));
console.log(b.languages()[0]);
