import { LRUCache } from 'lru-cache';

console.log('--- LRU Eviction & Recency ordering ---');
const cache = new LRUCache({ max: 3 });
cache.set('k1', 'v1');
cache.set('k2', 'v2');
cache.set('k3', 'v3');
// peek should not update recency
console.log('peek k1:', cache.peek('k1'));
// set 4th item -> k1 is oldest, so k1 should be evicted
cache.set('k4', 'v4');
console.log('has k1:', cache.has('k1'));
console.log('has k2:', cache.has('k2'));

// get k2 updates recency: now k3 is oldest
console.log('get k2:', cache.get('k2'));
cache.set('k5', 'v5');
console.log('has k3 (should be evicted):', cache.has('k3'));
console.log('has k2 (should remain):', cache.has('k2'));
console.log('keys in order (MRU to LRU):', [...cache.keys()].join(','));

console.log('--- Size-based eviction & dispose hook ---');
const evicted = [];
const sizeCache = new LRUCache({
  maxSize: 100,
  sizeCalculation: (val) => val.length,
  dispose: (val, key, reason) => {
    evicted.push(key + ':' + reason);
  }
});

sizeCache.set('docA', 'x'.repeat(40));
sizeCache.set('docB', 'y'.repeat(40));
console.log('current calculated size:', sizeCache.calculatedSize);
// Adding docC (30 bytes) exceeds 100 bytes (40+40+30=110), so docA is evicted
sizeCache.set('docC', 'z'.repeat(30));
console.log('evicted list:', evicted.join(', '));
console.log('has docA:', sizeCache.has('docA'));
console.log('has docB:', sizeCache.has('docB'));
console.log('has docC:', sizeCache.has('docC'));

console.log('--- Dump & Load ---');
const entries = cache.dump();
console.log('dumped entries count:', entries.length);
const restoredCache = new LRUCache({ max: 3 });
restoredCache.load(entries);
console.log('restored size:', restoredCache.size);
console.log('restored has k2:', restoredCache.has('k2'));
console.log('restored get k4:', restoredCache.get('k4'));

console.log('--- Clear & Delete ---');
console.log('delete k2:', restoredCache.delete('k2'));
console.log('has k2 after delete:', restoredCache.has('k2'));
restoredCache.clear();
console.log('size after clear:', restoredCache.size);
