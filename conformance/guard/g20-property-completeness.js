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

const hiddenIndex = [1, 2];
Object.defineProperty(hiddenIndex, '0', { enumerable: false });
console.log(Object.keys(hiddenIndex).join(','), Object.entries(hiddenIndex).map(x => x.join(':')).join(','), Object.values(hiddenIndex).join(','));
console.log(Reflect.ownKeys(hiddenIndex).map(String).join(','), JSON.stringify(hiddenIndex), Object.getOwnPropertyDescriptor(hiddenIndex, '0').enumerable);

const shifted = [1, 2, 3];
Object.defineProperty(shifted, '1', { enumerable: false });
shifted.shift();
const unshifted = [1, 2];
Object.defineProperty(unshifted, '1', { enumerable: false });
unshifted.unshift(0);
const spliced = [1, 2, 3];
Object.defineProperty(spliced, '1', { enumerable: false });
spliced.splice(0, 0, 9);
console.log(Object.keys(shifted).join(','), Object.keys(unshifted).join(','), Object.keys(spliced).join(','));

const shiftedZero = [1, 2];
Object.defineProperty(shiftedZero, '0', { enumerable: false });
shiftedZero.shift();
const unshiftedZero = [1];
Object.defineProperty(unshiftedZero, '0', { enumerable: false });
unshiftedZero.unshift(0);
console.log(Object.keys(shiftedZero).join(',') || '(none)', Object.keys(unshiftedZero).join(',') || '(none)');

const holes = [, 2];
Object.defineProperty(holes, '1', { enumerable: false });
holes.unshift(0);
holes[1] = 5;
const copied = [1, , 3];
Object.defineProperty(copied, '0', { enumerable: false });
copied.copyWithin(1, 0, 1);
const reversed = [1, , 3];
Object.defineProperty(reversed, '0', { enumerable: false });
reversed.reverse();
console.log(Object.keys(holes).join(','), Object.keys(copied).join(','), Object.keys(reversed).join(','));
