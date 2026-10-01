const EventEmitter = require('eventemitter3');

console.log('--- Context Binding & Multi-arguments ---');
const ee = new EventEmitter();
const logs = [];

const ctxObj = { prefix: '[CTX]' };
ee.on('log', function(level, msg) {
  logs.push(this.prefix + ' ' + level + ': ' + msg);
}, ctxObj);

ee.emit('log', 'INFO', 'starting up');
ee.emit('log', 'WARN', 'low memory');
console.log('logs:', JSON.stringify(logs));

console.log('--- once with Context ---');
let onceCount = 0;
ee.once('init', function(val) {
  onceCount += val + (this.offset || 0);
}, { offset: 50 });

ee.emit('init', 10);
ee.emit('init', 20); // should not fire
console.log('onceCount:', onceCount);

console.log('--- eventNames, listeners, listenerCount ---');
ee.on('status', () => {});
ee.on('data', () => {});
ee.on('data', () => {});
console.log('data listenerCount:', ee.listenerCount('data'));
console.log('eventNames has data:', ee.eventNames().includes('data'));
console.log('eventNames has status:', ee.eventNames().includes('status'));
console.log('listeners length:', ee.listeners('data').length);

console.log('--- Targeted removal with context ---');
const targetFn = function() {};
const ctx1 = { id: 1 };
const ctx2 = { id: 2 };
ee.on('target', targetFn, ctx1);
ee.on('target', targetFn, ctx2);
console.log('before remove:', ee.listenerCount('target'));
ee.removeListener('target', targetFn, ctx1);
console.log('after remove ctx1:', ee.listenerCount('target'));

console.log('--- removeAllListeners ---');
ee.removeAllListeners('target');
console.log('target listeners after removeAll:', ee.listenerCount('target'));
ee.removeAllListeners();
console.log('all eventNames after clear:', ee.eventNames().length);
