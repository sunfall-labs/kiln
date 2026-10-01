const mitt = require('mitt');

console.log('--- Multi-listener ordering & Unsubscribing ---');
const emitter = mitt();
const events = [];

const h1 = (data) => events.push('h1:' + data);
const h2 = (data) => events.push('h2:' + data);
const h3 = (data) => events.push('h3:' + data);

emitter.on('data', h1);
emitter.on('data', h2);
emitter.on('data', h3);
emitter.emit('data', 1);

// remove middle handler h2
emitter.off('data', h2);
emitter.emit('data', 2);
console.log('events:', JSON.stringify(events));

console.log('--- Self-unsubscribing listener ---');
const selfEvents = [];
const onceHandler = (msg) => {
  selfEvents.push('once:' + msg);
  emitter.off('once', onceHandler);
};
emitter.on('once', onceHandler);
emitter.emit('once', 'first');
emitter.emit('once', 'second');
console.log('selfEvents:', JSON.stringify(selfEvents));

console.log('--- Wildcard listeners ---');
const wildEvents = [];
emitter.on('*', (type, payload) => wildEvents.push(type + '->' + payload));
emitter.emit('user:login', 'alice');
emitter.emit('user:logout', 'alice');
console.log('wildEvents:', JSON.stringify(wildEvents));

console.log('--- Custom Map & emitter.all.clear() ---');
const customMap = new Map();
const customEmitter = mitt(customMap);
customEmitter.on('test', () => {});
console.log('customMap has test:', customMap.has('test'));
customEmitter.all.clear();
console.log('customMap size after clear:', customMap.size);

emitter.emit('no-listeners');
console.log('emit with no listeners ok');
