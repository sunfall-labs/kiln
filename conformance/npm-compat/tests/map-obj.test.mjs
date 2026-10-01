// kiln eval smoke test (authored for kiln, not from porffor).
import mapObject, { mapObjectSkip } from 'map-obj';

const upper = mapObject({ a: 1, b: 2 }, (k, v) => [k.toUpperCase(), v * 10]);
console.log('mapped:', JSON.stringify(upper));

const deep = mapObject({ a: { b: 1 } }, (k, v) => [k + '!', v], { deep: true });
console.log('deep:', JSON.stringify(deep));

const filtered = mapObject({ a: 1, b: 2, c: 3 }, (k, v) => (v % 2 === 0 ? [k, v] : mapObjectSkip));
console.log('skipped odd:', JSON.stringify(filtered));

const target = {};
mapObject({ x: 1 }, (k, v) => [k, v], { target });
console.log('target:', JSON.stringify(target));

const arrVals = mapObject({ list: [1, 2] }, (k, v) => [k, v], { deep: true });
console.log('array value:', JSON.stringify(arrVals));
