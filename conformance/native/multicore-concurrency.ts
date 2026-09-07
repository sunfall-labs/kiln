// Multicore concurrency native probe: parallelMap and pure closures.

// No polyfill: both engines answer `parallelMap` themselves, and this probe is the comparison.
const nums = [1, 2, 3, 4, 5];
const mapped = (nums as any).parallelMap((x: number) => x * 3);
console.log(mapped.join(","));

const pureCalc = (nums as any).parallelMap((x: number, i: number) => x + i);
console.log(pureCalc.join(","));

// Holes stay holes, the receiver is passed, and the answer is an array, not a promise.
const holey = [1, , 3] as any[];
const kept = holey.parallelMap((x: number, i: number, a: number[]) => x + i + a.length);
console.log(kept.length, 1 in kept, kept[0], kept[2], Array.isArray(kept));
