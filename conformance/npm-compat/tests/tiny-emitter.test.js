// kiln eval smoke test (authored for kiln, not from porffor).
const Emitter = require('tiny-emitter');
const e = new Emitter();
const out = [];
e.on('x', v => out.push('on:' + v));
e.once('y', v => out.push('once:' + v));
e.emit('x', 1);
e.emit('x', 2);
e.emit('y', 3);
e.emit('y', 4);
console.log(out.join(','));
e.off('x');
e.emit('x', 5);
console.log(out.join(','));
console.log(typeof e.on, typeof e.off, typeof e.emit, typeof e.once);
