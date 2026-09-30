// kiln eval smoke test (authored for kiln, not from porffor).
const Negotiator = require('negotiator');
const n = new Negotiator({ headers: { accept: 'text/html;q=0.8, application/json' } });
console.log(JSON.stringify(n.mediaTypes(['application/json', 'text/html'])));
console.log(n.mediaType(['application/json', 'text/html']));
const n2 = new Negotiator({ headers: { 'accept-charset': 'utf-8, iso-8859-1;q=0.5' } });
console.log(JSON.stringify(n2.charsets(['iso-8859-1', 'utf-8'])));
const n3 = new Negotiator({ headers: { 'accept-language': 'en-US, en;q=0.9' } });
console.log(JSON.stringify(n3.languages(['en', 'fr'])));
const n4 = new Negotiator({ headers: { 'accept-encoding': 'gzip, deflate' } });
console.log(JSON.stringify(n4.encodings(['gzip', 'identity'])));
