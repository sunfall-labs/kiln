const EventEmitter = require('node:events');
const Stream = require('node:stream');
function LegacyEmitter() { EventEmitter.call(this); }
Object.setPrototypeOf(LegacyEmitter.prototype, EventEmitter.prototype);
const emitter = new LegacyEmitter();
let seen = 0;
emitter.on('value', value => { seen += value; });
emitter.emit('value', 7);
console.log(seen, emitter instanceof EventEmitter, EventEmitter.EventEmitter === EventEmitter);
function LegacyStream() { Stream.call(this); }
Object.setPrototypeOf(LegacyStream.prototype, Stream.prototype);
const stream = new LegacyStream();
stream.on('value', value => { seen += value; });
stream.emit('value', 5);
console.log(seen, stream instanceof Stream, Stream.Stream === Stream);
class DerivedEmitter extends EventEmitter {}
class DerivedStream extends Stream {}
console.log(new DerivedEmitter() instanceof EventEmitter, new DerivedStream() instanceof Stream);
for (const constructor of [EventEmitter, Stream]) {
  try { constructor(); } catch (error) { console.log(error instanceof TypeError); }
}
