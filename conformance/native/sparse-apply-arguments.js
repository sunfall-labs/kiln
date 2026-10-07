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
