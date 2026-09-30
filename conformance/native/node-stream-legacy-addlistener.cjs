// Old readable-stream versions make `addListener` an alias of their `on` wrapper. The base
// EventEmitter `on` implementation must add directly, rather than dynamically calling it.
const Stream = require('node:stream');

function LegacyReadable() {
  Stream.call(this);
}
Object.setPrototypeOf(LegacyReadable.prototype, Stream.prototype);
LegacyReadable.prototype.on = function (event, listener) {
  return Stream.prototype.on.call(this, event, listener);
};
LegacyReadable.prototype.addListener = LegacyReadable.prototype.on;

const stream = new LegacyReadable();
let seen = 0;
stream.on('value', value => { seen += value; });
stream.emit('value', 7);
console.log(seen, stream instanceof Stream, Stream.prototype.on === Stream.prototype.addListener);
