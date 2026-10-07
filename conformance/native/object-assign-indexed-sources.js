// Indexed and symbol keys must copy through ordinary property reads and target setters.
const symbol = Symbol('extra');
const source = [7, , 9];
source.named = 'named';
source[symbol] = 'symbol';
Object.defineProperty(source, 'hidden', { value: 1 });
const target = {};
Object.assign(target, source, new Uint8Array([4]), 'A\ud800', 3, true, null, undefined);
console.log(JSON.stringify(target), target[symbol], Object.hasOwn(target, 'hidden'));
console.log(Reflect.ownKeys(source).map(key => typeof key === 'symbol' ? 'symbol' : key).join(','));
const changing = [1, 2, 3];
const copied = {};
Object.defineProperty(copied, '0', { set(value) { delete changing[1]; changing[3] = 4; } });
Object.assign(copied, changing);
console.log(JSON.stringify(copied));
const read = [5];
Object.defineProperty(read, 'extra', { enumerable: true, get() { return this[0] + 1; } });
console.log(JSON.stringify(Object.assign({}, read)));
const frozen = Object.freeze({ value: 1 });
try { Object.assign(frozen, { value: 2 }); } catch (error) { console.log(error instanceof TypeError); }
