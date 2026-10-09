// The same shapes with declarations that lie (`as any`): the call answers a string, the
// speculation leaves for the generic clone after the call that answered, and the program
// still prints what node prints. The call runs once -- its side effect is counted.
let calls = 0;

function pick(a: number[], i: number): number {
  calls++;
  return a[i];
}

function weigh(o: { w: number }): number {
  calls++;
  return o.w;
}

function total(a: number[]): number {
  let s: number = 0;
  for (let i = 0; i < a.length; i++) {
    s += pick(a, i) * 2 + 1;
  }
  return s;
}

function joined(a: number[]): number {
  let s: number = 0;
  for (let i = 0; i < a.length; i++) {
    s = s + pick(a, i);
  }
  return s;
}

function scaled(o: { w: number }, k: number): number {
  return weigh(o) + k;
}

const good: number[] = [];
for (let i = 0; i < 40; i++) good.push(i);
for (let r = 0; r < 30; r++) total(good);
const lying = [1, 2, "3", 4, undefined, null, { valueOf: () => 7 }, 8n === 8n, "x"] as any as number[];
console.log(total(lying));
console.log(joined(lying));
console.log(joined(good));
console.log(scaled({ w: "5" as any }, 1));
console.log(scaled({ w: 5 }, 1));
console.log(calls);
