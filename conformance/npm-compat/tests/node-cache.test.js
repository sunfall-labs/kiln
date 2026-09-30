// kiln eval smoke test (authored for kiln, not from porffor).
const NodeCache = require('node-cache');
const c = new NodeCache({stdTTL: 60, checkperiod: 0});
c.set('k', 'v');
console.log(c.get('k'), c.has('k'));
c.set('n', 42);
console.log(c.mget(['k', 'n', 'missing']).n);
console.log(c.getStats().keys);
c.del('k');
console.log(c.has('k'), c.keys().length);
console.log(JSON.stringify(c.keys()));
c.close();
