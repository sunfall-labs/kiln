// A Map or Set is shared as a sealed collection (ADR 0073, amended 2026-10-11): every read a live
// collection answers, from the reading realm's prototype, and a TypeError for every write.
import { share, isShared } from 'kiln:shared';

const owner = { name: 'acme' };
const source = new Map([
  ['acme', 2],
  [owner, 'object key'],
  [-0, 'zero'],
  [NaN, 'nan'],
  [12n, 'bigint'],
  [true, 'flag'],
  [null, 'null'],
  [undefined, 'undefined'],
  ['nested', new Map([['inner', [1, 2]]])],
]);
const shared = share(source);
source.set('acme', 3);
source.delete(owner);

console.log('brand', isShared(shared), shared instanceof Map, Object.prototype.toString.call(shared),
  shared.constructor === Map, Object.getPrototypeOf(shared) === Map.prototype);
console.log('size', shared.size, source.size);
console.log('get', shared.get('acme'), shared.get(0), shared.get(-0), shared.get(NaN), shared.get(12n),
  shared.get(true), shared.get(null), shared.get(undefined), shared.get('missing'), shared.get(owner));
console.log('has', shared.has('acme'), shared.has(-0), shared.has(owner), shared.has(12), shared.has('12'));
const sharedOwner = [...shared.keys()][1];
console.log('object key', isShared(sharedOwner), sharedOwner !== owner, sharedOwner.name, shared.get(sharedOwner),
  shared.has(sharedOwner), Object.isFrozen(sharedOwner));
console.log('order', [...shared.keys()].map(String).join(','));
console.log('entries', [...shared].map(([key, value]) => `${String(key)}=${String(value)}`).join(' '));
let visited = [];
shared.forEach(function (value, key, map) { visited.push(`${String(key)}:${map === shared}:${this.tag}`); }, { tag: 't' });
console.log('forEach', visited.join(' '));
const nested = shared.get('nested');
console.log('nested', isShared(nested), nested instanceof Map, nested.get('inner').join('+'),
  Array.isArray(nested.get('inner')), shared.get('nested') === nested);
console.log('object view', Object.keys(shared).length, Reflect.ownKeys(shared).length, JSON.stringify(shared),
  Object.isFrozen(shared), Object.isExtensible(shared), typeof shared);

function attempt(label, write) {
  try { write(); console.log(label, 'wrote'); } catch (error) { console.log(label, error.name, error.message); }
}
attempt('set', () => shared.set('acme', 9));
attempt('delete', () => shared.delete('acme'));
attempt('clear', () => shared.clear());
attempt('set.call', () => Map.prototype.set.call(shared, 'x', 1));
attempt('add on map', () => Set.prototype.add.call(shared, 1));
attempt('own property', () => { 'use strict'; shared.extra = 1; });
console.log('getOrInsert present', shared.getOrInsert('acme', 0), shared.getOrInsertComputed('acme', () => 0));
let computedRan = false;
attempt('getOrInsert absent', () => shared.getOrInsert('new', 1));
attempt('getOrInsertComputed absent', () => shared.getOrInsertComputed('new', () => { computedRan = true; return 1; }));
console.log('callback ran', computedRan);
const sizeGetter = Object.getOwnPropertyDescriptor(Map.prototype, 'size').get;
console.log('size getter', sizeGetter.call(shared));
console.log('after writes', shared.size, shared.get('acme'));

const letters = share(new Set(['a', 'b', -0, owner]));
console.log('set brand', letters instanceof Set, Object.prototype.toString.call(letters), letters.size,
  letters.has('a'), letters.has(0), letters.has('z'), letters.has(owner));
console.log('set iteration', [...letters].map(String).join(','), [...letters.entries()].map(([a, b]) => a === b).join(','));
console.log('set algebra', [...letters.union(new Set(['c']))].length, new Set(['a']).isSubsetOf(letters),
  letters.isSupersetOf(new Set(['b'])), [...letters.intersection(new Set(['a', 'q']))].join(','),
  letters.union(new Set()) instanceof Set, isShared(letters.union(new Set())));
attempt('set add', () => letters.add('c'));
attempt('set delete', () => letters.delete('a'));

const record = share({ lookup: new Map([['k', { v: 1 }]]), tags: new Set([1, 2]) });
console.log('inside a record', record.lookup.get('k').v, record.tags.has(2), JSON.stringify(record));
console.log('reshare', share(shared) === shared, share(new Map()) !== share(new Map()), share(new Map()).size);
// ADR 0073: a structured clone of a shared value is that value, as a message carries it.
console.log('structuredClone', structuredClone(shared) === shared, structuredClone({ m: letters }).m === letters);
const big = new Map();
for (let i = 0; i < 1000; i++) big.set(`key${i}`, i);
const sharedBig = share(big);
let sum = 0;
for (let i = 0; i < 1000; i++) sum += sharedBig.get(`key${i}`);
console.log('large', sharedBig.size, sum, sharedBig.get('key999'), sharedBig.has('key1000'));

function reject(label, value) {
  try { share(value); console.log(label, 'shared'); } catch (error) { console.log(label, error.name, error.message); }
}
class Registry extends Map {}
reject('subclass', new Registry());
const decorated = new Map(); decorated.label = 'x';
reject('own property', decorated);
reject('weak map', new WeakMap());
reject('function value', new Map([['f', () => 1]]));
reject('function in set', new Set([() => 1]));
