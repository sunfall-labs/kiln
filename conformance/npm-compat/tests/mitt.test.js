// kiln eval smoke test (authored for kiln, not from porffor).
const mitt = require('mitt');

const emitter = mitt();
const seen = [];
emitter.on('ping', (d) => seen.push('ping:' + d));
emitter.emit('ping', 'one');
emitter.emit('ping', 'two');
console.log('seen:', JSON.stringify(seen));

const allSeen = [];
emitter.on('*', (type, d) => allSeen.push(type + '=' + d));
emitter.emit('other', 'x');
console.log('wildcard:', JSON.stringify(allSeen));

const fn = () => seen.push('removed');
emitter.on('rm', fn);
emitter.off('rm', fn);
emitter.emit('rm');
console.log('after off:', JSON.stringify(seen));

console.log('all handlers map:', typeof emitter.all === 'object');
emitter.emit('no-listeners');
console.log('emit with no listeners ok');
