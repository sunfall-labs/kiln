import { EventEmitter } from 'node:events';

const emitter = new EventEmitter();
const observed = [];
let helloCalls = 0;
const hello = (a, b) => {
  if (a !== 'a' || b !== 'b') throw new Error('listener arguments changed');
  helloCalls += 1;
};

emitter.on('newListener', (event, listener) => {
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
console.log('pass');
