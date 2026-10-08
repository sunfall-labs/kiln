// A call to a function declared `: number` is guessed to answer a number where its result
// feeds arithmetic (invariant `call_return_number`). The declaration holds here.
function pick(a: number[], i: number): number {
  return a[i];
}

function weigh(o: { w: number }): number {
  return o.w;
}

function total(a: number[]): number {
  let s = 0;
  for (let i = 0; i < a.length; i++) {
    s += pick(a, i) * 2 + 1;
  }
  return s;
}

function scaled(o: { w: number }, k: number): number {
  return weigh(o) * k - 3;
}

const data: number[] = [];
for (let i = 0; i < 100; i++) data.push(i * 0.5);
let acc = 0;
for (let r = 0; r < 50; r++) acc += total(data);
console.log(acc);
let sc = 0;
for (let r = 0; r < 200; r++) sc += scaled({ w: r }, 1.5);
console.log(sc);
