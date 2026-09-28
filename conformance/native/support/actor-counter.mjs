import { Actor } from 'kiln:actors';

export class Counter extends Actor {
  constructor(initial) {
    super();
    this.count = initial;
  }
  increment(amount) {
    this.count += amount;
    return this.count;
  }
  async waitTurn() {
    await new Promise((resolve) => setTimeout(resolve, 1));
    return this.count;
  }
  echo(value) { return value; }
  growTransferredArray(value) {
    if (value.length !== 100000 || value[0] !== 3 || value[99999] !== 3) return null;
    value[200000] = 4;
    return [value.length, value[0], value[99999], value[200000]];
  }
  inspectGraph(value) {
    return [Object.getPrototypeOf(value) === Object.prototype, value.answer, value.values.join(',')];
  }
  sumLinked(value) {
    let count = 0;
    let sum = 0;
    while (value !== null) {
      count++;
      sum += value[0];
      value = value[1];
    }
    return [count, sum];
  }
  probeMailboxBind(endpoint) {
    const internals = MessagePort[Symbol.for('kiln.messagePort')];
    return internals.workerOp(33, endpoint);
  }
  waitRejected() {
    const cell = new Int32Array(new SharedArrayBuffer(4));
    try { return Atomics.wait(cell, 0, 0, 0); }
    catch (error) { return `${error.name}: ${error.message}`; }
  }
  fail() { throw new RangeError('remote problem'); }
}
