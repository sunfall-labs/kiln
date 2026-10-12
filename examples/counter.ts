import { Actor } from 'kiln:actors';

export class Counter extends Actor {
  private count: number;
  constructor(start: number) {
    super();
    this.count = start;
  }
  increment(delta: number): number {
    return (this.count += delta);
  }
  readOwner(record: { readonly owner: string }): string {
    return record.owner;
  }
}
