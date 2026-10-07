function f(first, second) { console.log(first, second, first || 'fallback'); }
const args = [, 2];
f.apply(null, args);
Reflect.apply(f, null, args);
Array.prototype[0] = 'inherited';
f.apply(null, args);
Reflect.apply(f, null, args);
delete Array.prototype[0];
const observed = [];
Object.defineProperty(observed, '0', { get() { console.log('get'); return 3; } });
observed.length = 2;
f.apply(null, observed);
Reflect.apply(f, null, observed);
function Constructed(first, second) { console.log('construct', first, second); }
Array.prototype[0] = 'inherited';
Reflect.construct(Constructed, args);
delete Array.prototype[0];
Reflect.construct(Constructed, observed);
const reads = [];
const list = {
  get length() { reads.push('length'); return { valueOf() { reads.push('number'); return 2; } }; },
  get 0() { reads.push('zero'); return 4; },
  get 1() { reads.push('one'); return 5; },
};
Reflect.construct(Constructed, list);
console.log(reads.join(','));
let entered = false;
try {
  Reflect.construct(function () { entered = true; }, { length: 1, get 0() { throw new RangeError('getter'); } });
} catch (error) { console.log(error instanceof RangeError, entered); }
