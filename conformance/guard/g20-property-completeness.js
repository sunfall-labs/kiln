// Typed own-key ordering, descriptor enumerability, callable own properties, and array
// iteration must all use the same observable property model.
const first = Symbol('first');
const second = Symbol('second');
const object = { b: 1, 2: 2, a: 3, 1: 4, [first]: 5, z: 6 };
Object.defineProperty(object, 'hidden', { value: 7 });
Object.defineProperty(object, second, { value: 8 });
console.log(Reflect.ownKeys(object).map(String).join(','));
console.log(Object.keys(object).join(','), Object.values(object).join(','), Object.entries(object).map(x => x.join(':')).join(','));
console.log(Object.getOwnPropertySymbols(object).length, object.hidden, object[second]);

function f() {}
f.visible = 1;
Object.defineProperty(f, 'hidden', { value: 2 });
f[first] = 3;
console.log('visible' in f, 'hidden' in f, Object.keys(f).join(','), Reflect.ownKeys(f).map(String).join(','));
console.log(({ ...f }).visible, Object.assign({}, f).visible, delete f.visible, 'visible' in f);

const array = [1, 2];
array[Symbol.iterator] = function* () { yield 7; yield 8; };
console.log([...array].join(','), Array.from(array).join(','));
