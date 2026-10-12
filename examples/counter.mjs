import { Actor } from 'kiln:actors';

export class Counter extends Actor {
  constructor(start) {
    super();
    this.count = start;
  }
  increment(delta) {
    this.count += delta;
    return this.count;
  }
  readOwner(record) {
    return record.owner;
  }
}
