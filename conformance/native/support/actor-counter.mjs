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
  holdGraph(value) { return new Promise(() => {}); }
  growTransferredArray(value) {
    if (value.length !== 100000 || value[0] !== 3 || value[99999] !== 3) return null;
    value[200000] = 4;
    return [value.length, value[0], value[99999], value[200000]];
  }
  growTransferredShape(value) {
    if (Object.keys(value).length !== 12 || value.s00 !== 0 || value.s11 !== 11) return null;
    value.s12 = 12; value.s13 = 13; value.s14 = 14; value.s15 = 15; value.s16 = 16;
    return [Object.keys(value).length, value.s00, value.s08, value.s16];
  }
  inspectGraph(value) {
    return [Object.getPrototypeOf(value) === Object.prototype, value.answer, value.values.join(',')];
  }
  inspectPreparedDictionary(value) {
    const key = Object.keys(value)[0];
    const original = value[key];
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    const nullPrototype = Object.getPrototypeOf(value) === null;
    value['received-' + key] = 23;
    const grew = value['received-' + key] === 23;
    return [key, original, typeof original, descriptor?.value, nullPrototype, grew,
            Object.keys(value).length];
  }
  inspectPreparedNumericObject(value) {
    const keys = Object.keys(value).join(',');
    const snapshot = [value[0], value[127].answer, value.label.answer];
    value[255] = 4;
    return [keys, ...snapshot, value[255], Object.keys(value).join(',')];
  }
  inspectNumericAfterAlloc(value) {
    const scratch = [];
    for (let index = 0; index < 4096; index++) scratch.push({ index });
    return [Object.keys(value).join(','), value[0], value[127], value.label, scratch.length];
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
    const internals = globalThis.__kilnPrivate.messagePort;
    return internals.workerOp(33, endpoint);
  }
  waitRejected() {
    const cell = new Int32Array(new SharedArrayBuffer(4));
    try { return Atomics.wait(cell, 0, 0, 0); }
    catch (error) { return `${error.name}: ${error.message}`; }
  }
  fail() { throw new RangeError('remote problem'); }
}
