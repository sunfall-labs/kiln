const symbol = Symbol('key');
const events = [];
Object.defineProperty(Object.prototype, 'inherited', {
  set() { throw new Error('inherited setter'); }, configurable: true
});
const entry = {
  get 0() { events.push('key'); return { [Symbol.toPrimitive](hint) { events.push(hint); return 'inherited'; } }; },
  get 1() { events.push('value'); return 42; }
};
const result = Object.fromEntries([['__proto__', 'value'], [symbol, 7], entry]);
delete Object.prototype.inherited;
console.log(Object.getPrototypeOf(result) === Object.prototype,
  Object.hasOwn(result, '__proto__'), result.__proto__, result[symbol], result.inherited);
console.log(events.join(','), JSON.stringify(Object.getOwnPropertyDescriptor(result, '__proto__')));
const map = new Map([['ignored', 0]]);
map[Symbol.iterator] = function*() { yield ['custom', 1]; };
console.log(JSON.stringify(Object.fromEntries(map)));
const closing = {
  *[Symbol.iterator]() { try { yield 1; } finally { events.push('closed'); } }
};
try { Object.fromEntries(closing); } catch (error) { console.log(error.name, events.at(-1)); }
console.log(Object.fromEntries.length);
