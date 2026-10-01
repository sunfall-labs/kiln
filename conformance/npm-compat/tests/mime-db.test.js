// kiln eval smoke test (authored for kiln, not from porffor).
const db = require('mime-db');
console.log(db['text/html'].extensions.join(','));
console.log(db['application/json'].extensions.join(','));
console.log(db['image/png'].source);
console.log(db['application/octet-stream'].extensions.slice(0, 3).join(','));
console.log(Object.keys(db).length > 1000);
