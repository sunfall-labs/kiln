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
  fail() { throw new RangeError('remote problem'); }
}
