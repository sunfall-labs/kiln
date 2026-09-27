// Membership indexing must preserve trap order, UTF-16 keys, symbol identity and invariants.
const symbol = Symbol('key');
const another = Symbol('key');
const target = Object.create(null);
for (const key of ['a', '\ud800', symbol, another])
  Object.defineProperty(target, key, { value: 1, configurable: false });
Object.preventExtensions(target);
const order = [another, '\ud800', symbol, 'a'];
const proxy = new Proxy(target, { ownKeys() { return order; } });
const actual = Reflect.ownKeys(proxy);
console.log(actual.length, actual.every((key, i) => key === order[i]));

function rejected(keys) {
  try {
    Reflect.ownKeys(new Proxy(target, { ownKeys() { return keys; } }));
    return false;
  } catch (error) {
    return error instanceof TypeError;
  }
}
console.log(rejected(order.concat('extra')), rejected(order.slice(1)), rejected(order.concat(symbol)));

// CreateListFromArrayLike reads all entries before duplicate-key validation.
const sentinel = {};
let reads = '';
try {
  Reflect.ownKeys(new Proxy({}, { ownKeys() {
    return { length: 3, get 0() { reads += '0'; return 'same'; },
      get 1() { reads += '1'; return 'same'; }, get 2() { reads += '2'; throw sentinel; } };
  } }));
} catch (error) {
  console.log(reads, error === sentinel);
}
