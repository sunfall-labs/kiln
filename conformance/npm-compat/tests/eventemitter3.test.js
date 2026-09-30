// kiln eval smoke test (authored for kiln, not from porffor).
const EventEmitter = require('eventemitter3');
const ee = new EventEmitter();
let sum = 0;
ee.on('add', n => { sum += n; });
ee.once('once', () => { sum += 100; });
ee.emit('add', 5);
ee.emit('add', 7);
ee.emit('once');
ee.emit('once');
console.log(sum);
console.log(ee.listenerCount('add'));
ee.off('add');
console.log(ee.listenerCount('add'));
ee.emit('add', 1000);
console.log(sum);
