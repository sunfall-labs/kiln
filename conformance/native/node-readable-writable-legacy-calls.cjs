const { Readable, Writable } = require('node:stream');

function LegacyReadable() {
  Readable.call(this, { objectMode: true });
}
Object.setPrototypeOf(LegacyReadable.prototype, Readable.prototype);
const readable = new LegacyReadable();
readable.push(7);
readable.push(null);
console.log(readable instanceof Readable, typeof readable.on, readable.read());

function LegacyWritable() {
  Writable.call(this, { objectMode: true });
}
Object.setPrototypeOf(LegacyWritable.prototype, Writable.prototype);
const writable = new LegacyWritable();
writable.write(9);
writable.end();
console.log(writable instanceof Writable, typeof writable.setMaxListeners, writable.writableFinished);
