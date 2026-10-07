const { EventEmitter } = require('node:events');
console.log(Math.random.prototype === undefined, Object.hasOwn(Math.random, 'prototype'));
Function.prototype.prototype = 17;
console.log(Math.random.prototype, EventEmitter.prototype !== 17);
Math.random.prototype = 23;
console.log(Math.random.prototype, Object.hasOwn(Math.random, 'prototype'));
delete Math.random.prototype;
console.log(Math.random.prototype);
Object.defineProperty(Math.random, 'prototype', {
  configurable: true,
  get() { return this === Math.random ? 31 : 0; }
});
console.log(Math.random.prototype);
delete Math.random.prototype;
delete Function.prototype.prototype;
console.log(Math.random.prototype === undefined);
