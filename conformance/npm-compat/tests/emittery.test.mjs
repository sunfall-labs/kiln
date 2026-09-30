// kiln eval smoke test (authored for kiln, not from porffor).
import Emittery from 'emittery';

const ee = new Emittery();
const seen = [];
ee.on('data', ({ data }) => seen.push('on:' + data));
await ee.emit('data', 42);
console.log('seen:', JSON.stringify(seen));

const onceSeen = [];
ee.once('once', ({ data }) => onceSeen.push(data));
await ee.emit('once', 'a');
await ee.emit('once', 'b');
console.log('once:', JSON.stringify(onceSeen));

const off = () => seen.push('x');
ee.on('rm', off);
await ee.off('rm', off);
await ee.emit('rm');
console.log('after off, seen:', JSON.stringify(seen));

console.log('listenerCount:', ee.listenerCount('data'));
console.log('listenerCount 1/0:', ee.listenerCount('data'), ee.listenerCount('nope'));

const serial = [];
ee.on('s', async ({ data }) => { serial.push(data); });
await ee.emitSerial('s', 'one');
await ee.emitSerial('s', 'two');
console.log('serial:', JSON.stringify(serial));
