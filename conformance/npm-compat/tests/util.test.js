// kiln eval smoke test (authored for kiln, not from porffor).
const util = require('util');

console.log('format:', util.format('%s has %d apples', 'ann', 5));
console.log('format obj:', util.format('obj: %j', { a: 1 }));
console.log('inspect:', util.inspect({ a: [1, 2, { b: 'x' }] }, { depth: 2 }));
console.log('isDeepStrictEqual:', util.isDeepStrictEqual({ a: [1] }, { a: [1] }), util.isDeepStrictEqual({ a: 1 }, { a: 2 }));

function Base() { this.x = 1; }
Base.prototype.hello = function () { return 'hi'; };
function Child() { Base.call(this); }
util.inherits(Child, Base);
const kid = new Child();
console.log('inherits:', kid instanceof Base, kid.hello(), kid.x);

const promised = util.promisify((a, b, cb) => cb(null, a + b));
promised(2, 3).then((v) => console.log('promisify:', v));

console.log('deprecate is fn:', typeof util.deprecate(() => {}, 'old') === 'function');
console.log('types isArrayBuffer:', util.types.isArrayBuffer(new ArrayBuffer(8)));
