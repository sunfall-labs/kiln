const { EventEmitterAsyncResource } = require('node:events');
const emitter = new EventEmitterAsyncResource({ name: 'worker-probe' });
let observed = '';
emitter.on('work', (value) => { observed = value; });
console.log(typeof emitter.asyncId, typeof emitter.triggerAsyncId);
console.log(emitter.asyncResource.eventEmitter === emitter);
console.log(emitter.emit('work', 'done'), observed);
console.log(emitter.emitDestroy() === undefined);
