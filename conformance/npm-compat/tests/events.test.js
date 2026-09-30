// kiln eval smoke test (authored for kiln, not from porffor).
const { EventEmitter } = require('events');

const ee = new EventEmitter();
const seen = [];
ee.on('data', (x) => seen.push('on:' + x));
ee.once('once', (x) => seen.push('once:' + x));
ee.emit('data', 1);
ee.emit('data', 2);
ee.emit('once', 'a');
ee.emit('once', 'b');
console.log('seen:', JSON.stringify(seen));
console.log('listenerCount data:', ee.listenerCount('data'));

const off = () => seen.push('removed');
ee.on('rm', off);
ee.removeListener('rm', off);
ee.emit('rm');
console.log('after remove, seen length:', seen.length);

console.log('maxListeners default:', ee.getMaxListeners());
ee.setMaxListeners(5);
console.log('maxListeners set:', ee.getMaxListeners());

const names = [];
ee.on('x', () => {});
ee.on('y', () => {});
console.log('eventNames:', JSON.stringify(ee.eventNames().sort()));
