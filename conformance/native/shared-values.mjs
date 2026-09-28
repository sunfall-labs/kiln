import {share, isShared} from 'kiln:shared';
const a = {v: 7};
const source = {left:a, right:a};
source.self = source;
Object.defineProperty(source, 'hidden', {value: 9, enumerable:false});
const list = [1, , a]; list.note = 'n';
source.list = list;
const shared = share(source);
a.v = 8;
console.log('identity', shared.left === shared.right, shared.self === shared, shared.left.v, share(shared) === shared, share({}) !== share({}));
console.log('array', Array.isArray(shared.list), shared.list.length, 1 in shared.list, shared.list.note, shared.list[2] === shared.left, shared.list instanceof Array);
console.log('keys', Object.keys(shared).join(','), Reflect.ownKeys(shared.list).join(','), Object.getOwnPropertyDescriptor(shared,'hidden').enumerable, Object.getOwnPropertyDescriptor(shared,'hidden').writable);
console.log('proto', Object.getPrototypeOf(shared) === Object.prototype, Object.getPrototypeOf(shared.list) === Array.prototype, Object.isFrozen(shared), Object.isFrozen(shared.left), Object.isFrozen(shared.list));
const plain = share({a:[1,2]});
console.log('view', JSON.stringify(plain), [...shared.list].join('|'), ({...shared}).left === shared.left);
let write = 'ok'; try { shared.left.v = 1; } catch (error) { write = error.name; }
let define = 'ok'; try { Object.defineProperty(shared, 'new', {value:1}); } catch (error) { define = error.name; }
console.log('mutation', write, define, Reflect.set(shared, 'new', 1), Reflect.deleteProperty(shared, 'left'));
console.log('weak', new WeakMap([[shared, 3]]).get(shared));
function reject(value) { try { share(value); console.log('bad'); } catch (error) { console.log('reject', error.name); } }
reject(function(){}); reject(new Date()); reject({get x(){return 1}}); reject(new Proxy({},{})); reject({[Symbol('s')]:1}); reject(Object.create({a:1}));
