// Type contracts guide guarded entries; JavaScript callers can still pass another type,
// replace a method or install a getter. Authored method signatures survive TS lowering.
type Count = number;
class Counter {
  step(value: Count): Count { return value + 1; }
  twice(value: Count): Count { return value * 2; }
}
const counter: any = new Counter();
for (let index = 0; index < 3; index++) {
  console.log(counter.step(index), counter.step("x"), counter.twice("3"));
}
const alternate = { step(value: string): string { return value + "!"; } };
function invoke(receiver: any, value: any): any { return receiver.step(value); }
console.log(invoke(counter, 3), invoke(alternate, "x"));
counter.step = (value: any) => "replacement:" + value;
console.log(invoke(counter, 4));
let reads = 0;
Object.defineProperty(counter, "step", {
  get() { reads++; return (value: any) => "getter:" + value; },
});
console.log(invoke(counter, 5), reads);
