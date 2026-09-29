import { Actor } from 'kiln:actors';
import { isShared } from 'kiln:shared';

export class SharedReader extends Actor {
  constructor(value) {
    super();
    this.initial = value;
  }

  inspect(value = this.initial) {
    return {
      shared: isShared(value),
      nested: isShared(value.records),
      length: value.records.length,
      sum: value.records[0].n + value.records[1].n + value.records[2].n,
      alias: value.alias === value.records[1],
    };
  }

  echo(value) { return value; }
}
