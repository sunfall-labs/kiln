// Type-only wrappers preserve the Reference and evaluate receiver/key/value once.
const events: string[] = [];
let stored = 4;
const receiver = {
  get value() { events.push('get'); return stored; },
  set value(next: number) { events.push('set:' + next); stored = next; },
  method<T>(value: T) { return this.value + Number(value); }
};
const object = () => { events.push('receiver'); return receiver; };
const key = () => { events.push('key'); return 'value'; };
const rhs = () => { events.push('rhs'); return 10; };
console.log(object()[key()]!++);
console.log(++object()[key()]!);
console.log((object()[key()] as number) += rhs());
let count: number | undefined = 1;
(count as number)! += 2;
(count as number)!++;
console.log(count);
console.log(receiver.method<number>(2));
console.log(events.join(','));
