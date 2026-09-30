const { Readable, Writable, Duplex, Transform } = require('node:stream');

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

function LegacyDuplex() {
  Duplex.call(this, { objectMode: true });
}
Object.setPrototypeOf(LegacyDuplex.prototype, Duplex.prototype);
const duplex = new LegacyDuplex();
duplex.push(11);
console.log(duplex instanceof Duplex, duplex.read());

function LegacyTransform() {
  Transform.call(this, { objectMode: true, transform(chunk, _encoding, callback) { callback(null, chunk * 2); } });
}
Object.setPrototypeOf(LegacyTransform.prototype, Transform.prototype);
const transform = new LegacyTransform();
transform.write(6);
console.log(transform instanceof Transform, transform.read());
