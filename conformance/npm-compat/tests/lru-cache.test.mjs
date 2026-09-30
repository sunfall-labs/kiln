// kiln eval smoke test (authored for kiln, not from porffor).
import { LRUCache } from 'lru-cache';

const c = new LRUCache({ max: 2 });
c.set('a', 1); c.set('b', 2); c.set('c', 3);
console.log(c.has('a'), c.get('b'), c.get('c'));
console.log([...c.keys()].join(','));
c.set('d', 4, { ttl: 60000 });
console.log(c.size, c.has('d'));
console.log(c.get('missing', { allowStale: true }));
