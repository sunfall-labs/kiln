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
  echo(value) { return value; }
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
  fail() { throw new RangeError('remote problem'); }
}
