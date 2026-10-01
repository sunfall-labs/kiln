import * as namespace from './fixtures/module-namespace-values-entries-source.mjs';
import { update } from './fixtures/module-namespace-values-entries-source.mjs';

console.log(Object.values(namespace).filter(value => typeof value !== 'function').join(','));
console.log(Object.values(namespace).includes(update));
console.log(Object.entries(namespace)
    .filter(([, value]) => typeof value !== 'function')
    .map(([key, value]) => `${key}:${value}`)
    .join(','));
console.log(Object.entries(namespace).find(([key]) => key === 'update')[1] === update);
update(42);
console.log(Object.values(namespace).filter(value => typeof value !== 'function').join(','));
console.log(Object.values(namespace).includes(update));
console.log(Object.entries(namespace)
    .filter(([, value]) => typeof value !== 'function')
    .map(([key, value]) => `${key}:${value}`)
    .join(','));
console.log(Object.entries(namespace).find(([key]) => key === 'update')[1] === update);
