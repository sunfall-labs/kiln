// Compact prologues must retain receiver, capture, exception, and suspension state on demand.
function identity(value) { return value; }
console.log(identity(17), identity());

const receiver = {
  base: 40,
  read(extra) { return () => this.base + extra; },
  *values() { yield this.base; return this.base + 2; },
  async delayed() { await Promise.resolve(); return this.base + 3; },
};
console.log(receiver.read(1)());
const sequence = receiver.values();
console.log(sequence.next().value, sequence.next().value);

function fail(value) { throw value; }
try { fail(44); } catch (error) { console.log(error); }

function capturedArgument(value) { return () => arguments[0]; }
console.log(capturedArgument(45)());

class Box {
  constructor(value) { this.value = value; }
  read() { return this.value; }
}
console.log(new Box(46).read());
receiver.delayed().then(value => console.log(value));
