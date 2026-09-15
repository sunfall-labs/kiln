// `parallelMap` on both engines (ADR 0058, application 2). The native backend runs a callback it
// can prove reads its double parameters and returns a double on the provider's pool; every other
// callback takes the sequential lowering. Either way the answer is the interpreter's, which is
// what this probe compares. Nothing here is refused.

// 1. A qualifying numeric callback over enough elements to clear the fork-join floor.
const big: number[] = [];
for (let i = 0; i < 100000; i++) big.push(i);
const squared = (big as any).parallelMap((x: number) => Math.sqrt(x * x + 1) * 2 - Math.floor(x / 3));
let checksum = 0;
for (let i = 0; i < squared.length; i++) checksum += squared[i];
console.log("qualifying", squared.length, checksum);

// 2. With the index parameter.
const indexed = (big as any).parallelMap((x: number, i: number) => x - i + Math.abs(-i));
console.log("indexed", indexed.length, indexed[0], indexed[99999]);

// 3. A callback that allocates: sequential, and the answer is arrays.
const boxed = ([1, 2, 3] as any).parallelMap((x: number) => [x]);
console.log("allocates", boxed.length, Array.isArray(boxed[0]), boxed[2][0]);

// 4. A callback reading a heap upvalue: sequential.
const table = { scale: 10 };
const scaled = ([1, 2, 3] as any).parallelMap((x: number) => x * table.scale);
console.log("upvalue", scaled.join(","));

// 5. A hole: the receiver is not numeric, so sequential, and the hole stays a hole.
const holey = [1, , 3] as any[];
const kept = holey.parallelMap((x: number) => x * 2);
console.log("hole", kept.length, 1 in kept, kept[0], kept[2]);

// 6. A callback returning a string: sequential, and the answer is strings.
const named = ([1, 2, 3] as any).parallelMap((x: number) => "n" + x);
console.log("string", named.join("|"));

// 7. A small numeric array, below the floor, still through the same lowering.
const small = ([3, 1, 2] as any).parallelMap((x: number) => x * x);
console.log("small", small.join(","));

// 8. Transcendental and trigonometric math builtins in parallelMap.
const bigTrig: number[] = [];
for (let i = 0; i < 50000; i++) bigTrig.push(i);
const trig = (bigTrig as any).parallelMap((x: number) => {
    return Math.sin(x) + Math.cos(x) + Math.hypot(x, 1) + Math.log(x + 1) + Math.exp(-x);
});
let trigSum = 0;
for (let i = 0; i < 100; i++) trigSum += Math.round(trig[i] * 1000);
console.log("transcendental", trig.length, trigSum);

