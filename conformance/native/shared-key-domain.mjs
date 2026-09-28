import {share} from 'kiln:shared';
const symbol = Symbol.iterator;
const object = {[symbol]: 3, '@@iterator': 4, '@@symbol:1': 5, '@@entries': 6};
console.log('reads', object[symbol], object['@@iterator'], object['@@symbol:1'], object['@@entries']);
console.log('keys', Object.keys(object).join(','), Object.getOwnPropertyNames(object).join(','), Object.getOwnPropertySymbols(object).length);
console.log('reflect', Reflect.ownKeys(object).length, typeof Reflect.ownKeys(object)[3]);
console.log('values', Object.values(object).join(','), JSON.stringify(object));
try { share(object); console.log('bad'); } catch (error) { console.log('reject', error.name); }
const plain = {'@@iterator': 4, '@@symbol:1': 5, '@@entries': 6};
const sealed = share(plain);
console.log('shared', Object.keys(sealed).join(','), Reflect.ownKeys(sealed).length, JSON.stringify(sealed));
