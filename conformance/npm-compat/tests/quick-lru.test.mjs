// kiln eval smoke test (authored for kiln, not from porffor).
import QuickLRU from 'quick-lru';

const cache = new QuickLRU({ maxSize: 3 });
cache.set('a', 1);
cache.set('b', 2);
cache.set('c', 3);
console.log('get a:', cache.get('a'));
console.log('has b:', cache.has('b'), 'size:', cache.size);
cache.set('d', 4);
console.log('after evict, has a:', cache.has('a'), 'has d:', cache.has('d'));
cache.delete('b');
console.log('after delete, has b:', cache.has('b'), 'size:', cache.size);
cache.set('e', 5);
cache.set('f', 6);
console.log('keys:', JSON.stringify([...cache.keys()].sort()));
console.log('values sum:', [...cache.values()].reduce((s, v) => s + v, 0));
cache.clear();
console.log('after clear size:', cache.size);
console.log('get missing:', cache.get('zzz'));
