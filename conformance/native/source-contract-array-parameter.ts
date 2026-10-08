// A declared array parameter is a guess the checker hands the emitter; the entry guard checks the
// Array kind, and every other value must still run the generic body and print what node prints.
function sum(values: number[]): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) total += values[i];
  return total;
}
function last(values: readonly string[]): string | undefined {
  return values.length === 0 ? undefined : values[values.length - 1];
}
function firstTwo(pair: [number, number]): number {
  return pair.length === 2 ? pair[0] + pair[1] : -1;
}
function reassigned(values: number[]): number {
  values = values.length > 1 ? [values[0]] : [];
  return values.length;
}

console.log(sum([1, 2, 3, 4]));
console.log(sum([]));
console.log(sum([1.5, 2.5, -4]));
console.log(last(["a", "b", "c"]));
console.log(last([]));
console.log(firstTwo([3, 4]));
console.log(reassigned([5, 6, 7]));

// The lies: a declaration says array and the value is not one (or not a plain one).
function attempt(thunk: () => unknown): void {
  try {
    console.log(thunk());
  } catch (error) {
    console.log("threw", (error as Error).constructor.name);
  }
}
attempt(() => sum({ length: 3, 0: 10, 1: 20, 2: 30 } as any));
attempt(() => sum("123" as any));
attempt(() => sum(new Float64Array([1, 2, 3]) as any));
attempt(() => sum(new Uint8Array([9, 9]) as any));
attempt(() => sum(undefined as any));
attempt(() => sum(null as any));
attempt(() => sum(5 as any));
attempt(() => last("xyz" as any));
attempt(() => firstTwo({ length: 2, 0: 1, 1: 2 } as any));
class Sub extends Array<number> {}
attempt(() => sum(Sub.from([7, 8, 9])));
const holey: number[] = [1, , 3] as any;
attempt(() => sum(holey));
attempt(() => sum(new Proxy([1, 2, 3], {}) as any));
for (let round = 0; round < 3; round++) {
  attempt(() => sum(round === 1 ? ([4, 5] as any) : ("no" as any)));
}
