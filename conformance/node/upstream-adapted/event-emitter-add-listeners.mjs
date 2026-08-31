import { EventEmitter } from 'node:events';

const emitter = new EventEmitter();
if (emitter.addListener !== emitter.on) {
  throw new Error('addListener is not the same function as on');
}
const observed = [];
let helloCalls = 0;
const hello = (a, b) => {
  if (a !== 'a' || b !== 'b') throw new Error('listener arguments changed');
  helloCalls += 1;
};

emitter.on('newListener', function (event, listener) {
  if (this !== emitter) throw new Error('newListener receiver changed');
  if (event === 'newListener') return;
  if (emitter.listeners(event).length !== 0) {
    throw new Error('newListener ran after registration');
  }
  observed.push(event);
  if (event === 'hello' && listener !== hello) {
    throw new Error('newListener received the wrong listener');
  }
});
emitter.on('hello', hello);
emitter.once('foo', () => { throw new Error('foo listener ran'); });

if (observed.length !== 2 || observed[0] !== 'hello' || observed[1] !== 'foo') {
  throw new Error('newListener ordering changed');
}
emitter.emit('hello', 'a', 'b');
if (helloCalls !== 1) throw new Error('hello listener did not run exactly once');
emitter.setMaxListeners(0);

const reentrant = new EventEmitter();
const listen1 = () => {};
const listen2 = () => {};
let outerNewListenerCalls = 0;
let innerNewListenerCalls = 0;

reentrant.once('newListener', () => {
  outerNewListenerCalls += 1;
  if (reentrant.listeners('hello').length !== 0) {
    throw new Error('outer newListener observed hello too early');
  }
  reentrant.once('newListener', () => {
    innerNewListenerCalls += 1;
    if (reentrant.listeners('hello').length !== 0) {
      throw new Error('inner newListener observed hello too early');
    }
  });
  reentrant.on('hello', listen2);
});
reentrant.on('hello', listen1);

const helloListeners = reentrant.listeners('hello');
if (outerNewListenerCalls !== 1 || innerNewListenerCalls !== 1) {
  throw new Error('re-entrant newListener callbacks did not run exactly once');
}
if (helloListeners.length !== 2 || helloListeners[0] !== listen2 || helloListeners[1] !== listen1) {
  throw new Error('re-entrant newListener changed listener insertion order');
}
console.log('pass');
